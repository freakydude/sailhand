use super::*;
use serde_json::json;

fn store(state: &str, extra: Value) -> ObjectStore {
    let mut store = ObjectStore::default();
    let mut objects = json!({
        "print_stats": { "state": state, "filename": "benchy.gcode", "info": { "current_layer": 3 } },
        "extruder": { "temperature": 209.8, "target": 210.0, "power": 0.4 },
        "motion_report": { "live_extruder_velocity": 2.5, "live_velocity": 120.0 },
        "gcode_move": { "speed_factor": 1.0, "extrude_factor": 1.0 },
        "virtual_sdcard": { "file_position": 1234 },
        "configfile": { "settings": { "extruder": {
            "filament_diameter": 1.75, "nozzle_diameter": 0.4, "rotation_distance": 22.6, "pressure_advance": 0.04 } } },
        "mmu": {
            "encoder": null, "sensors": { "filament_proportional": false }, "gate": 1, "tool": 1, "happy_hare_version": "4.0.1",
            "gate_material": ["PLA", "PETG"], "gate_color": ["ff0000", "00ff00"], "gate_spool_id": [-1, 7],
            "sync_feedback_flow_rate": 98.0, "sync_feedback_flow_rate_calibrated": 96.5,
            "sync_feedback_bias_raw": 0.1, "flowguard": { "level": 0.2, "trigger": "" }
        }
    });
    // The extra fields replace single fields of an object, not the object.
    for (name, fields) in extra.as_object().cloned().unwrap_or_default() {
        objects[name.as_str()].as_object_mut().unwrap().extend(fields.as_object().cloned().unwrap());
    }
    store.replace(&objects);
    store
}

fn update(recorder: &mut FlowRecorder, store: &ObjectStore, object: &str, now_ms: u64) {
    let status = PrinterStatus::from_store(store);
    recorder.observe(store, &status, &json!({ object: {} }), now_ms);
}

/// The cells of a CSV line, with quoted cells whole.
fn cells(line: &str) -> Vec<String> {
    let (mut cells, mut cell, mut quoted) = (Vec::new(), String::new(), false);
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => cells.push(std::mem::take(&mut cell)),
            c => cell.push(c),
        }
    }
    cells.push(cell);
    cells
}

fn facts() -> HeaderFacts {
    let spool = Spool {
        id: 7,
        vendor: "Acme".into(),
        filament_name: "Blue".into(),
        material: "PETG".into(),
        ..Spool::default()
    };
    HeaderFacts {
        job_id: Some("0001AB".into()),
        happy_hare_version: Some("v4.0.1".into()),
        spools: HashMap::from([(7, spool)]),
        ..HeaderFacts::default()
    }
}

fn recorder(directory: &Path) -> FlowRecorder {
    FlowRecorder::new(Some(FlowRecording { directory: directory.to_owned(), printer: "My Voron".into() }))
}

fn recorded_files(directory: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(directory).unwrap().flatten().map(|entry| entry.path()).collect();
    files.sort();
    files
}

#[test]
fn records_header_rows_events_and_the_end_of_a_print() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder = recorder(directory.path());
    update(&mut recorder, &store("standby", json!({})), "print_stats", 1_000);
    assert!(!recorder.wants_facts());

    let printing = store("printing", json!({}));
    update(&mut recorder, &printing, "print_stats", 2_000);
    assert!(recorder.wants_facts());
    assert!(!recorder.wants_facts(), "asked once per print");
    recorder.begin(&printing, &facts(), 1_800_000_000_000);

    update(&mut recorder, &printing, "extruder", 2_250);
    update(&mut recorder, &store("paused", json!({})), "print_stats", 3_000);
    let resumed = store("printing", json!({ "mmu": { "gate": 2, "tool": 2 }, "extruder": { "target": 230.0 } }));
    update(&mut recorder, &resumed, "mmu", 4_000);
    let complete = store("complete", json!({ "mmu": { "gate": 2, "tool": 2 }, "extruder": { "target": 230.0 } }));
    update(&mut recorder, &complete, "print_stats", 5_000);
    update(&mut recorder, &complete, "print_stats", 5_500);

    let files = recorded_files(directory.path());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name().unwrap(), "flow-20270115-080000-My_Voron-0001AB.csv");
    let text = fs::read_to_string(&files[0]).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.contains(&"# job_id: 0001AB"));
    assert!(lines.contains(&"# file: benchy.gcode"));
    assert!(lines.contains(&"# extruder_rotation_distance: 22.6"));
    assert!(lines.contains(&"# extruder_pressure_advance: 0.04"));
    assert!(lines.contains(&"# happy_hare_version: v4.0.1"));
    assert!(lines.contains(&"# mmu_sensor: proportional (PSF)"));
    assert!(lines.contains(&"# gate_1: material=PETG color=00ff00 spool_id=7 spoolman_filament=Acme Blue PETG"));
    let columns = lines.iter().position(|line| line.starts_with("t_ms,")).unwrap();
    let width = cells(lines[columns]).len();
    assert!(lines[columns + 1..].iter().all(|line| cells(line).len() == width), "every row has all columns");
    let row = lines[columns + 1..].iter().find(|line| line.contains(",row,")).unwrap();
    assert!(row.starts_with("0,row,2.5,120.0,96.5,98.0,96.5,"), "{row}");
    let events: Vec<&str> = lines[columns + 1..].iter().filter(|line| line.contains(",event,")).copied().collect();
    let names: Vec<String> = events.iter().map(|line| cells(line).remove(width - 2)).collect();
    assert_eq!(
        names,
        ["flow_origin", "pause", "resume", "gate_change", "tool_change", "target_change", "end"],
        "{events:?}"
    );
    assert!(events.last().unwrap().ends_with(",end,complete"));
    assert!(events.iter().any(|line| line.ends_with(",gate_change,1>2")));
    assert!(text.contains(",flow_origin,\"estimated sync feedback with proportional sensor, against"));
    assert!(text.ends_with("complete\n"), "nothing after the end");
}

#[test]
fn a_print_that_ends_before_the_header_arrives_still_ends_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder = recorder(directory.path());
    update(&mut recorder, &store("printing", json!({})), "extruder", 1_000);
    update(&mut recorder, &store("cancelled", json!({})), "print_stats", 2_000);
    assert!(!recorder.wants_facts());
    recorder.begin(&store("cancelled", json!({})), &HeaderFacts::default(), 1_800_000_000_000);
    let files = recorded_files(directory.path());
    assert_eq!(files[0].file_name().unwrap(), "flow-20270115-080000-My_Voron-unknown.csv");
    assert!(fs::read_to_string(&files[0]).unwrap().trim_end().ends_with(",end,cancelled"));
}

#[test]
fn a_second_print_gets_its_own_file() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder = recorder(directory.path());
    for (start, job) in [(1_800_000_000_000, "A"), (1_800_000_100_000, "B")] {
        let printing = store("printing", json!({}));
        update(&mut recorder, &printing, "extruder", start);
        assert!(recorder.wants_facts());
        recorder.begin(&printing, &HeaderFacts { job_id: Some(job.into()), ..HeaderFacts::default() }, start);
        update(&mut recorder, &store("complete", json!({})), "print_stats", start + 10);
    }
    assert_eq!(recorded_files(directory.path()).len(), 2);
}

#[test]
fn keeps_only_the_newest_prints() {
    let directory = tempfile::tempdir().unwrap();
    for day in 1..=PRINTS_KEPT + 4 {
        fs::write(directory.path().join(format!("{FILE_PREFIX}202601{day:02}-000000-p-1{FILE_SUFFIX}")), "").unwrap();
    }
    fs::write(directory.path().join("sailhand.log"), "").unwrap();
    let mut recorder = recorder(directory.path());
    let printing = store("printing", json!({}));
    update(&mut recorder, &printing, "extruder", 1_000);
    recorder.wants_facts();
    recorder.begin(&printing, &facts(), 1_800_000_000_000);
    let names: Vec<String> = recorded_files(directory.path())
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.iter().filter(|name| name.starts_with(FILE_PREFIX)).count(), PRINTS_KEPT);
    assert!(names.contains(&"sailhand.log".to_owned()));
    assert!(names.iter().any(|name| name.contains("0001AB")), "the new one stays");
    assert!(!names.iter().any(|name| name.contains("20260101")), "the oldest goes");
}

#[test]
fn without_the_setting_nothing_is_written() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder = FlowRecorder::new(None);
    assert!(!recorder.enabled());
    update(&mut recorder, &store("printing", json!({})), "extruder", 1_000);
    assert!(!recorder.wants_facts());
    assert!(recorded_files(directory.path()).is_empty());
}

#[test]
fn updates_of_other_objects_make_no_row() {
    let directory = tempfile::tempdir().unwrap();
    let mut recorder = recorder(directory.path());
    let printing = store("printing", json!({}));
    update(&mut recorder, &printing, "fan", 1_000);
    recorder.wants_facts();
    recorder.begin(&printing, &facts(), 1_800_000_000_000);
    update(&mut recorder, &printing, "heater_bed", 1_250);
    update(&mut recorder, &printing, "extruder", 1_500);
    let text = fs::read_to_string(&recorded_files(directory.path())[0]).unwrap();
    let rows = text.lines().filter(|line| line.contains(",row,")).count();
    assert_eq!(rows, 1);
}

#[test]
fn dates_come_out_in_utc() {
    assert_eq!(utc_stamp(0), "19700101-000000");
    assert_eq!(utc_stamp(1_800_000_000_000), "20270115-080000");
    assert_eq!(utc_stamp(951_782_400_000 + 86_399_000), "20000229-235959");
}
