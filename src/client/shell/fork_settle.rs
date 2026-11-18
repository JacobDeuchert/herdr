//! Close a thread pane only after the configured settlement helper acknowledges it.
use super::*;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

impl ClientShellState {
    fn settle_target_valid(&self, target: &ClientTabCloseConfirmation, thread: &str) -> bool {
        target.workspace.endpoint_id == self.active_endpoint_id
            && self.navigation_target_valid(&target.workspace)
            && target.pane_id.as_deref().is_some_and(|id| {
                self.settle_thread_for_pane(id).as_deref() == Some(thread)
                    && self.snapshot.as_deref().is_some_and(|s| {
                        s.panes.iter().any(|p| {
                            p.pane_id == id
                                && p.tab_id == target.tab_id
                                && p.workspace_id == target.workspace.workspace_id
                        })
                    })
            })
    }

    pub(super) fn accept_settle_close(&mut self, outcome: &mut ClientShellInput) {
        if !matches!(&self.overlay, Some(ClientShellOverlay::ConfirmClose(c)) if c.thread.is_some())
        {
            return;
        }
        let Some(ClientShellOverlay::ConfirmClose(c)) = self.overlay.take() else {
            return;
        };
        let (Some(thread), Some(target)) = (c.thread, c.tab_target) else {
            return;
        };
        outcome.repaint = true;
        if self.settling.is_some() {
            self.receive_endpoint_unavailable("A thread settlement is already in progress".into());
            return;
        }
        if !self.settle_target_valid(&target, &thread) {
            self.receive_endpoint_unavailable(
                "Thread or pane changed; try closing it again".into(),
            );
            return;
        }
        let Some((program, args)) = self.config.threads.settle_command.split_first() else {
            return;
        };
        let mut command = Command::new(program);
        crate::platform::configure_background_command(&mut command);
        match command
            .args(args)
            .arg(&thread)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.settling = Some((
                    target,
                    thread,
                    child,
                    Instant::now() + Duration::from_secs(25),
                ))
            }
            Err(e) => {
                self.receive_endpoint_unavailable(format!("Could not start settle command: {e}"));
            }
        }
    }

    pub(super) fn poll_settle_close(&mut self, outcome: &mut ClientShellInput) {
        let Some((_, _, child, deadline)) = &mut self.settling else {
            return;
        };
        let success = match child.try_wait() {
            Ok(None) if Instant::now() < *deadline => return,
            Ok(Some(status)) => status.success(),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                false
            }
        };
        let Some((target, thread, _, _)) = self.settling.take() else {
            return;
        };
        outcome.repaint = true;
        if !success {
            self.receive_endpoint_unavailable("Settlement not confirmed; pane left open".into());
        } else if !self.settle_target_valid(&target, &thread) {
            self.receive_endpoint_unavailable("Thread settled; close target changed".into());
        } else if let Some(pane_id) = target.pane_id {
            self.push_endpoint_method(
                crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget { pane_id }),
                outcome,
            );
        }
    }
}
