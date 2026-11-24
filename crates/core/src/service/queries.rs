//! Queries the display asks Moonraker for: files, history, Spoolman, power devices and more.

use super::session::Session;
use super::{Event, QUERY_TIMEOUT, Query};
use crate::moonraker::access::{self};
use crate::moonraker::{Call, Connection, Endpoint};
use crate::sources::moonraker_server::files::{self, FileMetadata, ThumbnailImage};
use crate::sources::moonraker_server::gcode_store;
use crate::sources::moonraker_server::history;
use crate::sources::moonraker_server::machine;
use crate::sources::moonraker_server::power;
use crate::sources::moonraker_server::server_info::ServerComponents;
use crate::sources::moonraker_server::temperature_store;
use crate::sources::moonraker_server::update_manager;
use crate::sources::spoolman;
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

impl Session<'_> {
    pub(super) fn answer(&self, query: Query) {
        if let Some(component) = missing_component(&query, &self.components) {
            log::debug!("{query:?} not asked: Moonraker has no {component} component");
            return;
        }
        let connection = self.connection.clone();
        let downloads = Downloads {
            http: self.http.clone(),
            endpoint: self.endpoint.clone(),
            authorized: self.access.is_authorized(),
        };
        let events = self.outputs.events.clone();
        let gate = self.queries.pass();
        let lane_spools = self.spoolman_lane_spools.clone().unwrap_or_default();
        let file_details = matches!(query, Query::FileDetails { .. });
        let permits = file_details.then(|| self.file_details.clone());
        // A file's name only goes to the verbose log.
        let failure_level = if file_details { log::Level::Debug } else { log::Level::Info };
        self.queries.spawn(async move {
            // The timeout starts once it is this query's turn.
            let _permit = match permits {
                Some(permits) => permits.acquire_owned().await.ok(),
                None => None,
            };
            let fetched =
                tokio::time::timeout(QUERY_TIMEOUT, fetch(&connection, &downloads, &query, &lane_spools)).await;
            match fetched {
                Ok(Ok(results)) => gate.send(&events, results),
                Ok(Err(error)) => log::log!(failure_level, "query {query:?} failed: {error}"),
                Err(_) => log::log!(failure_level, "query {query:?} timed out"),
            }
        });
    }
}

/// The queries of one session. Their answers pass only while the session lives: after a switch to another printer (or
/// a reconnect) a late answer would land in the caches of the one shown now. Ending the session also stops the
/// queries still running.
pub(super) struct SessionQueries {
    open: Arc<Mutex<bool>>,
    tasks: Mutex<JoinSet<()>>,
}

impl SessionQueries {
    pub(super) fn new() -> Self {
        Self { open: Arc::new(Mutex::new(true)), tasks: Mutex::default() }
    }

    fn pass(&self) -> AnswerPass {
        AnswerPass(self.open.clone())
    }

    fn spawn(&self, query: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        while tasks.try_join_next().is_some() {}
        tasks.spawn(query);
    }
}

impl Drop for SessionQueries {
    fn drop(&mut self) {
        *lock(&self.open) = false;
    }
}

/// Lets a query's answer through while its session lives.
struct AnswerPass(Arc<Mutex<bool>>);

impl AnswerPass {
    /// Holds the lock while sending, so an answer is either queued before the session ends (and thus before the next
    /// session's events) or not at all.
    fn send(&self, events: &mpsc::UnboundedSender<Event>, answer: Vec<Event>) {
        let open = lock(&self.0);
        if *open {
            answer.into_iter().for_each(|event| {
                let _ = events.send(event);
            });
        }
    }
}

fn lock(open: &Mutex<bool>) -> MutexGuard<'_, bool> {
    open.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The Moonraker component a query needs, when this Moonraker has not loaded it; without it the method is unknown.
fn missing_component(query: &Query, components: &ServerComponents) -> Option<&'static str> {
    match query {
        Query::PrintHistory if !components.history => Some("history"),
        Query::Spoolman | Query::SpoolList if !components.spoolman => Some("spoolman"),
        Query::PowerDevices if !components.power => Some("power"),
        Query::Updates if !components.update_manager => Some("update_manager"),
        _ => None,
    }
}

/// Files over HTTP (thumbnails); an authorized display shows a one-time token from its connection with each.
struct Downloads {
    http: reqwest::Client,
    endpoint: Endpoint,
    authorized: bool,
}

impl Downloads {
    async fn get(&self, connection: &Connection, path: &str) -> Result<Vec<u8>, String> {
        let mut url = self.endpoint.http_url(path);
        if self.authorized {
            let token = access::oneshot_token(connection).await.map_err(|error| error.to_string())?;
            url = format!("{url}?token={token}");
        }
        let response = self.http.get(url).send().await.map_err(|error| error.to_string())?;
        let response = response.error_for_status().map_err(|error| error.to_string())?;
        Ok(response.bytes().await.map_err(|error| error.to_string())?.to_vec())
    }
}

async fn fetch(
    connection: &Connection,
    downloads: &Downloads,
    query: &Query,
    lane_spools: &[u32],
) -> Result<Vec<Event>, String> {
    let call =
        |call: Call| async move { connection.call(call.method, call.params).await.map_err(|error| error.to_string()) };
    let events = match query {
        Query::Files => vec![Event::Files(files::parse_file_list(&call(files::list_call()).await?))],
        Query::FileDetails { filename } => {
            let result = call(files::metadata_call(filename)).await?;
            let metadata = files::parse_metadata(&result).ok_or("unreadable metadata")?;
            let mut events = Vec::new();
            if let Some(path) = metadata.thumbnail_path() {
                match downloads.get(connection, &path).await {
                    Ok(data) => match ThumbnailImage::decode(&data) {
                        Some(image) => events.push(Event::Thumbnail { filename: filename.clone(), image }),
                        None => log::debug!("thumbnail of {filename} cannot be decoded"),
                    },
                    Err(error) => log::debug!("thumbnail of {filename} unavailable: {error}"),
                }
            }
            events.insert(0, Event::FileMetadata(Box::new(FileMetadata { filename: filename.clone(), ..metadata })));
            events
        }
        Query::PrintHistory => {
            vec![Event::PrintHistory(history::parse_print_history(&call(history::list_call()).await?))]
        }
        Query::Spoolman => vec![Event::Spoolman(Box::new(spoolman::fetch_state(connection, lane_spools).await?))],
        Query::SpoolList => vec![Event::SpoolList(spoolman::fetch_spool_list(connection).await?)],
        Query::PowerDevices => {
            vec![Event::PowerDevices(power::parse_power_devices(&call(power::devices_call()).await?))]
        }
        Query::Updates => update_status_events(&call(update_manager::status_call()).await?),
        Query::ConsoleHistory => {
            vec![Event::ConsoleHistory(gcode_store::parse_gcode_store(&call(gcode_store::history_call()).await?))]
        }
        Query::HostNetwork => {
            vec![Event::HostNetwork(machine::parse_host_network(&call(machine::system_info_call()).await?))]
        }
        Query::TemperatureHistory => vec![Event::TemperatureHistory(temperature_store::parse_temperature_store(
            &call(temperature_store::store_call()).await?,
        ))],
    };
    Ok(events)
}

/// The versions and whether the update manager is busy, from `machine.update.status` or `notify_update_refreshed`.
pub(super) fn update_status_events(status: &Value) -> Vec<Event> {
    vec![
        Event::Updates(update_manager::parse_update_status(status)),
        Event::UpdateManagerBusy(update_manager::is_busy(status)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Voron's Moonraker has no `[power]` section and answered `machine.device_power.devices` with
    /// "Method not found".
    #[test]
    fn a_query_waits_for_its_component() {
        let none = ServerComponents::default();
        assert_eq!(missing_component(&Query::PowerDevices, &none), Some("power"));
        assert_eq!(missing_component(&Query::PrintHistory, &none), Some("history"));
        assert_eq!(missing_component(&Query::SpoolList, &none), Some("spoolman"));
        assert_eq!(missing_component(&Query::Updates, &none), Some("update_manager"));
        assert_eq!(missing_component(&Query::Files, &none), None);

        let power = ServerComponents { power: true, ..ServerComponents::default() };
        assert_eq!(missing_component(&Query::PowerDevices, &power), None);
    }

    #[tokio::test]
    async fn answers_pass_only_while_the_session_lives() {
        let (events, mut received) = mpsc::unbounded_channel();
        let queries = SessionQueries::new();
        let pass = queries.pass();
        pass.send(&events, vec![Event::SpoolmanChanged]);

        drop(queries);
        pass.send(&events, vec![Event::SpoolmanChanged]);

        assert_eq!(received.try_recv(), Ok(Event::SpoolmanChanged));
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn ending_the_session_stops_its_queries() {
        let queries = SessionQueries::new();
        let (finished, was_finished) = tokio::sync::oneshot::channel::<()>();
        queries.spawn(async move {
            std::future::pending::<()>().await;
            let _ = finished.send(());
        });

        drop(queries);

        assert!(was_finished.await.is_err(), "the query should have been aborted");
    }
}
