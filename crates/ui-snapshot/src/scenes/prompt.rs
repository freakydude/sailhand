//! The dialog of a macro prompt (`action:prompt_*`) that the fake's demo macros send.

use super::STATUS_BAR_LIGHT;
use crate::session::Session;
use sailhand::{Actions, Navigation, Page, Printer, PromptDialog};
use sailhand_core::Command;
use sailhand_core::sources::klipper::PrintState;
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// The example of the issue: a question with two footer buttons (`prompt-dialog`); a tap on the status bar leaves it
/// open; the button's answer ends it. A second prompt with rows of buttons (`prompt-dialog-buttons`) is closed with
/// its cross, which has to reach Klipper and the dialog.
pub(super) fn macro_prompt(session: &mut Session) -> Result<(), String> {
    session.window.global::<Navigation>().set_page(Page::Print);
    run_demo(session, "DEMO_BED_CLEAR_PROMPT")?;
    let dialog = session.window.global::<PromptDialog>();
    if dialog.get_title() != "Is the bed empty?" || dialog.get_footer().row_count() != 2 {
        return Err("the bed prompt shows the wrong title or footer buttons".into());
    }
    session.capture("prompt-dialog")?;
    if session.has_reference_layout() {
        let (light_x, light_y) = STATUS_BAR_LIGHT;
        session.tap(light_x, light_y);
        session.window.global::<Navigation>().set_light_open(false);
        session.pump(Duration::from_millis(300));
        expect_open(session, true, "a tap on the status bar")?;
    }
    // The primary button of the footer: what its press sends, and the macro answers by ending the prompt.
    let yes = session.window.global::<PromptDialog>().get_footer().row_data(1).ok_or("no second footer button")?;
    session.window.global::<Actions>().invoke_run_gcode(yes.gcode);
    session.pump(Duration::from_millis(600));
    expect_open(session, false, "the answer of the macro (prompt_end)")?;

    run_demo(session, "DEMO_BUTTONS_PROMPT")?;
    session.capture("prompt-dialog-buttons")?;
    session.window.global::<Actions>().invoke_close_prompt();
    expect_open(session, false, "the cross")?;
    // The `prompt_end` the cross sends comes back from the fake; the dialog stays closed.
    session.pump(Duration::from_millis(600));
    expect_open(session, false, "the answer to the cross")
}

fn run_demo(session: &mut Session, name: &str) -> Result<(), String> {
    session.service.send(Command::RunMacro { name: name.into() });
    session.pump(Duration::from_millis(800));
    expect_open(session, true, name)
}

fn expect_open(session: &mut Session, open: bool, after: &str) -> Result<(), String> {
    if session.window.global::<PromptDialog>().get_open() == open {
        return Ok(());
    }
    Err(format!("the prompt dialog should be {} after {after}", if open { "open" } else { "closed" }))
}

/// Happy Hare's pause dialog (the fake's `FAKE_MMU_PAUSE`): with only its standard buttons it stays out, and its
/// extra line stands in the pause banner (`print-mmu-notice`); with a button of its own it shows as a dialog
/// (`prompt-dialog-happy-hare-buttons`). Skipped without Happy Hare; leaves the print running.
pub(super) fn happy_hare_pause_notice(session: &mut Session) -> Result<(), String> {
    if session.presenter.snapshot().status.happy_hare().is_none() {
        return Ok(());
    }
    session.window.global::<Navigation>().set_page(Page::Print);
    mmu_pause(session, "FAKE_MMU_PAUSE MSG=1")?;
    expect_open(session, false, "Happy Hare's notice with its standard buttons")?;
    if !session.window.global::<Printer>().get_interruption_text().contains("Spare spools are in the drawer") {
        return Err("the pause banner lacks the line of Happy Hare's notice".into());
    }
    session.capture("print-mmu-notice")?;
    resume(session)?;

    mmu_pause(session, "FAKE_MMU_PAUSE MSG=1 BUTTONS=1")?;
    expect_open(session, true, "Happy Hare's notice with a button of its own")?;
    session.capture("prompt-dialog-happy-hare-buttons")?;
    session.window.global::<Actions>().invoke_close_prompt();
    resume(session)
}

fn mmu_pause(session: &mut Session, script: &str) -> Result<(), String> {
    session.service.send(Command::RunGcode { script: script.into() });
    session.wait_until("paused by Happy Hare", |snapshot| snapshot.status.job.state == PrintState::Paused)?;
    // The clock refreshes the banner every second.
    session.pump(Duration::from_millis(1200));
    Ok(())
}

fn resume(session: &mut Session) -> Result<(), String> {
    session.service.send(Command::ResumePrint);
    session.wait_until("print resumed", |snapshot| snapshot.status.job.state == PrintState::Printing)
}
