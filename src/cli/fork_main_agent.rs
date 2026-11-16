//! fork: `herdr main-agent` (FORK.md 0004).
//!
//! `show` asks the attached client to show the `[ui.main_agent]` workspace,
//! wherever it is (never toggles away). `view` reports what the client is
//! showing. Both talk to the local server; `SUPER+G` in nixos-work uses them.

use clap::{ArgMatches, Command};

use crate::api::schema::{EmptyParams, Method, Request};

use super::projection::{self, ParseError};

pub(crate) fn command() -> Command {
    Command::new("main-agent")
        .about("Show the pinned Main agent in the attached client")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("show").about("Show Main in the attached client"))
        .subcommand(Command::new("view").about("Report what the attached client is showing"))
}

fn request(matches: &ArgMatches) -> Result<Request, ParseError> {
    match matches.subcommand() {
        Some(("show", _)) => Ok(Request {
            id: "cli:main-agent:show".into(),
            method: Method::ClientMainAgentShow(EmptyParams::default()),
        }),
        Some(("view", _)) => Ok(Request {
            id: "cli:main-agent:view".into(),
            method: Method::ClientViewGet(EmptyParams::default()),
        }),
        _ => Err(ParseError::Usage("expected main-agent show or view".into())),
    }
}

pub(super) fn run(args: &[String]) -> std::io::Result<i32> {
    match projection::parse(command(), args, request) {
        Ok(request) => super::print_response(&super::send_request(&request)?),
        Err(ParseError::Clap(error)) => super::print_clap_error(&error),
        Err(ParseError::Usage(message)) => {
            eprintln!("{message}");
            Ok(2)
        }
        Err(ParseError::Io(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_agent_cli_builds_requests() {
        for (arg, expected) in [
            (
                "show",
                serde_json::json!({
                    "id": "cli:main-agent:show",
                    "method": "client.main_agent.show",
                    "params": {},
                }),
            ),
            (
                "view",
                serde_json::json!({
                    "id": "cli:main-agent:view",
                    "method": "client.view.get",
                    "params": {},
                }),
            ),
        ] {
            let args = vec![arg.to_string()];
            let actual = projection::parse(command(), &args, request).expect("valid command");
            assert_eq!(serde_json::to_value(actual).unwrap(), expected);
        }
    }
}
