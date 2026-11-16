//! fork: server half of `herdr main-agent` (FORK.md 0004).
//!
//! Showing Main is a client action: only the client knows which machine it is
//! presenting. `client.main_agent.show` therefore forwards an endpoint control
//! to the client shell, which runs the same code as the pinned row. The
//! client accepts endpoint controls from the local server even while it is
//! showing another machine, so this works from any view.
//!
//! `client.view.get` answers "what is the client showing" from state the
//! server already tracks (`shell_surface_active` and the client's shell
//! location), so callers do not have to parse the outer window title.

use super::*;

impl HeadlessServer {
    /// The client shell the Main-agent API talks to: the foreground client,
    /// else the most recently active connected shell client. The latter may be
    /// presenting another machine, in which case it is not the foreground.
    fn fork_main_agent_client(&self) -> Option<u64> {
        let connected = |client: &crate::server::clients::ClientConnection| {
            client.is_shell_client() && client.writer.is_some()
        };
        self.foreground_client_id
            .filter(|id| self.clients.get(id).is_some_and(connected))
            .or_else(|| {
                self.clients
                    .iter()
                    .filter(|(_, client)| connected(client))
                    .max_by_key(|(_, client)| client.last_activity)
                    .map(|(&id, _)| id)
            })
    }

    pub(super) fn handle_client_main_agent_show_api(&mut self, id: String) -> String {
        let sent = self.fork_main_agent_client().is_some_and(|client_id| {
            self.send_to_client(
                client_id,
                ServerMessage::EndpointControl {
                    kind: crate::protocol::endpoint::FORK_MAIN_AGENT_SHOW_KIND.into(),
                    data: String::new(),
                },
            )
        });
        serde_json::to_string(&api::schema::SuccessResponse {
            id,
            result: api::schema::ResponseResult::ClientMainAgentShow { sent },
        })
        .unwrap_or_else(|_| "{}".to_string())
    }

    pub(super) fn handle_client_view_get_api(&mut self, id: String) -> String {
        let client_id = self.fork_main_agent_client();
        let active = client_id
            .and_then(|client_id| self.clients.get(&client_id))
            .is_some_and(|client| client.shell_surface_active);
        let workspace_index = client_id
            .and_then(|client_id| self.shell_target_for_client(client_id))
            .map(|target| target.workspace_index);
        let workspace_label = workspace_index
            .and_then(|index| self.app.state.workspaces.get(index))
            .map(|workspace| {
                workspace.display_name_from(&self.app.state.terminals, &self.app.terminal_runtimes)
            });
        let workspace_id = workspace_label
            .as_ref()
            .and(workspace_index)
            .map(|index| self.app.public_workspace_id(index));
        serde_json::to_string(&api::schema::SuccessResponse {
            id,
            result: api::schema::ResponseResult::ClientView {
                attached: client_id.is_some(),
                active,
                workspace_id,
                workspace_label,
            },
        })
        .unwrap_or_else(|_| "{}".to_string())
    }
}
