//! The tool macros `T0` … `Tn` and the variables they may declare by the convention of Fluidd and Mainsail. Like every
//! macro they are black boxes; only a variable declared under exactly this name is read (see `docs/printer-setup.md`,
//! "Spools of tool heads").
//!
//! `spool_id` holds the Spoolman spool of a tool: both clients assign it with `SET_GCODE_VARIABLE` and, if the printer
//! has `[save_variables]`, keep it with `SAVE_VARIABLE` under `t<n>__spool_id`. `active` marks the tool in use: Fluidd
//! also sets the active spool of Moonraker when it assigns a spool to a macro whose `active` is true
//! (`SpoolSelectionDialog.vue`, master 6e8b380). The user's macros set `active`; Sailhand only reads it.

use super::ObjectStore;
use super::klipper::Setup;
use super::subscription::Subscription;
use serde_json::Value;

const SPOOL_VARIABLE: &str = "spool_id";
const ACTIVE_VARIABLE: &str = "active";
const MACRO_PREFIX: &str = "gcode_macro ";

/// A tool macro that declares `spool_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpool {
    pub tool: usize,
    /// The macro name as written in the configuration (`T0` or `t0`): `SET_GCODE_VARIABLE` matches it exactly.
    pub macro_name: String,
    /// `None` when the variable is `None` (no spool assigned).
    pub spool_id: Option<u32>,
    /// The macro declares `active` and it is true.
    pub active: bool,
}

/// The number `n` of a tool macro `T<n>`, in any case.
fn tool_number(macro_name: &str) -> Option<usize> {
    let number = macro_name.strip_prefix(['T', 't'])?;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())).then(|| number.parse().ok())?
}

pub(super) fn subscribe(klipper: &Setup, subscription: &mut Subscription) {
    for name in klipper.macros.iter().filter(|name| tool_number(name).is_some()) {
        subscription.add_any_case(&format!("{MACRO_PREFIX}{name}"), Some(&[SPOOL_VARIABLE, ACTIVE_VARIABLE]));
    }
}

/// The tool macros that declare `spool_id`, by tool number.
pub(super) fn read_spools(store: &ObjectStore) -> Vec<ToolSpool> {
    let mut spools: Vec<ToolSpool> = store
        .names()
        .filter_map(|object| {
            let (kind, name) = object.split_at_checked(MACRO_PREFIX.len())?;
            let tool = tool_number(name).filter(|_| kind.eq_ignore_ascii_case(MACRO_PREFIX))?;
            let variables = store.object(object)?;
            let spool_id = variables.get(SPOOL_VARIABLE)?;
            Some(ToolSpool {
                tool,
                macro_name: name.to_owned(),
                spool_id: spool_id.as_u64().and_then(|id| u32::try_from(id).ok()),
                active: variables.get(ACTIVE_VARIABLE).is_some_and(is_truthy),
            })
        })
        .collect();
    spools.sort_by_key(|spool| spool.tool);
    spools
}

/// True as Fluidd reads the variable in JavaScript: `true`, a number other than 0, a text that is not empty.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
        Value::Null => false,
    }
}

/// The G-code that assigns `spool_id` (or none) to the tool macro `macro_name`, as Fluidd and Mainsail send it; with
/// `[save_variables]` also kept across restarts (Klipper wants that name in lower case).
pub fn assign_spool(macro_name: &str, spool_id: Option<u32>, save_variables: bool) -> String {
    let value = spool_id.map_or_else(|| "None".to_owned(), |id| id.to_string());
    let assign = format!("SET_GCODE_VARIABLE MACRO={macro_name} VARIABLE={SPOOL_VARIABLE} VALUE={value}");
    if save_variables {
        let saved = macro_name.to_ascii_lowercase();
        format!("{assign}\nSAVE_VARIABLE VARIABLE={saved}__{SPOOL_VARIABLE} VALUE={value}")
    } else {
        assign
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spools(status: Value) -> Vec<ToolSpool> {
        let mut store = ObjectStore::default();
        store.replace(&status);
        read_spools(&store)
    }

    #[test]
    fn only_tool_macros_that_declare_the_variable_hold_a_spool() {
        let found = spools(json!({
            "gcode_macro T1": { "spool_id": 7, "active": true },
            "gcode_macro t0": { "spool_id": null },
            "gcode_macro T2": {},
            "gcode_macro TURN_OFF": { "spool_id": 3 },
            "gcode_macro PARK": { "spool_id": 4 },
        }));

        assert_eq!(
            found,
            [
                ToolSpool { tool: 0, macro_name: "t0".into(), spool_id: None, active: false },
                ToolSpool { tool: 1, macro_name: "T1".into(), spool_id: Some(7), active: true },
            ]
        );
    }

    #[test]
    fn active_is_true_as_fluidd_reads_it() {
        let truthy: Vec<bool> = [json!(true), json!(1), json!("False"), json!(false), json!(0), json!(""), Value::Null]
            .iter()
            .map(is_truthy)
            .collect();
        assert_eq!(truthy, [true, true, true, false, false, false, false]);
    }

    #[test]
    fn assigning_sends_what_fluidd_and_mainsail_send() {
        assert_eq!(assign_spool("T2", Some(12), false), "SET_GCODE_VARIABLE MACRO=T2 VARIABLE=spool_id VALUE=12");
        assert_eq!(
            assign_spool("t0", None, true),
            "SET_GCODE_VARIABLE MACRO=t0 VARIABLE=spool_id VALUE=None\nSAVE_VARIABLE VARIABLE=t0__spool_id VALUE=None"
        );
    }
}
