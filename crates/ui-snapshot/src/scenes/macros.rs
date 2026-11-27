//! Motion › Macros: the fake's demo macro answers and then takes a while; the run card shows it running with the
//! answers so far (`motion-macros-running`), then done (`motion-macros-done`). What stood on the card is not unread.
//! Which macros the tab lists is chosen under System › Settings › Macros (`settings-macros`).

use crate::session::Session;
use sailhand::{Actions, MacroRunState, MotionTab, Navigation, Page, Place, Printer};
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// `DEMO_HEAT_SOAK` of the fake: two answers, then a dwell of 2 s.
const DEMO_MACRO: &str = "DEMO_HEAT_SOAK";

pub(super) fn macros(session: &mut Session) -> Result<(), String> {
    session.window.global::<Navigation>().set_motion_tab(MotionTab::Macros);
    session.window.global::<Navigation>().set_page(Page::Motion);
    session.window.global::<Actions>().invoke_select_macro(DEMO_MACRO.into());
    session.pump(Duration::from_millis(100));
    expect_state(session, MacroRunState::None, "before the first run")?;
    let unread_before = session.window.global::<Printer>().get_unread_count();

    session.window.global::<Actions>().invoke_run_macro(DEMO_MACRO.into());
    session.pump(Duration::from_millis(700));
    expect_state(session, MacroRunState::Running, "while it dwells")?;
    let answers = session.window.global::<Printer>().get_macro_run_lines().row_count();
    if answers != 2 {
        return Err(format!("the run card shows {answers} answers instead of 2"));
    }
    session.capture("motion-macros-running")?;

    session.pump(Duration::from_millis(2000));
    expect_state(session, MacroRunState::Done, "after its answer")?;
    if session.window.global::<Printer>().get_unread_count() != unread_before {
        return Err("answers that stood on the run card count as unread".into());
    }
    session.capture("motion-macros-done")?;
    session.window.global::<Navigation>().set_motion_tab(MotionTab::Axes);

    session.window.global::<Navigation>().invoke_go(Place::MacroSettings, "".into());
    session.pump(Duration::from_millis(300));
    session.capture("settings-macros")
}

fn expect_state(session: &Session, state: MacroRunState, when: &str) -> Result<(), String> {
    let shown = session.window.global::<Printer>().get_macro_run().state;
    if shown == state { Ok(()) } else { Err(format!("the run card shows {shown:?} {when}, expected {state:?}")) }
}
