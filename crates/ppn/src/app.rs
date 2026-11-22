use pepino_app::{App, Date, OpenOptions, ReadRequest, Request, Response, TaskSummary};
use tokio::runtime::{Builder, Runtime};

use crate::error::{CliError, ErrorKind};
use crate::response::ResponseExt;
use crate::terminal::Context;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvocationTime {
    pub now_ms: u64,
    pub today: Date,
    pub offset: chrono::FixedOffset,
}

pub struct AppClient {
    app: App,
    wrote: std::rc::Rc<std::cell::Cell<bool>>,
    board_notice: std::rc::Rc<std::cell::RefCell<Option<crate::output::Warning>>>,
    runtime: Runtime,
    now_ms: u64,
    board_override: Result<Option<(String, &'static str)>, CliError>,
}

impl AppClient {
    pub fn open(context: &Context<'_>) -> Result<Self, CliError> {
        Self::startup(context, OpenOptions::default(), false)
    }

    pub fn init(context: &Context<'_>, options: OpenOptions) -> Result<Self, CliError> {
        Self::startup(context, options, true)
    }

    fn startup(
        context: &Context<'_>,
        mut options: OpenOptions,
        initialize: bool,
    ) -> Result<Self, CliError> {
        let config = &context.config()?.config;
        let directory = config.data_dir.value.as_path();
        options.credentials_file = Some(config.credentials_file.value.as_path().to_owned());
        if let Some(timeout) = context.timeout {
            options.lock_timeout = timeout;
        }
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                CliError::new(
                    ErrorKind::Failure,
                    format!("Could not start the app: {error}"),
                )
            })?;
        let now_ms = context.time.now_ms;
        let app = runtime.block_on(async {
            if initialize {
                App::init_with_options(directory, now_ms, options).await
            } else {
                App::open_with_options(directory, now_ms, options).await
            }
        })?;
        if initialize {
            context.wrote.set(true);
        }
        Ok(Self {
            app,
            board_override: if initialize {
                Ok(None)
            } else {
                context
                    .board_override()
                    .map(|board| board.map(|(reference, source)| (reference.into(), source)))
            },
            wrote: context.wrote.clone(),
            board_notice: context.board_notice.clone(),
            runtime,
            now_ms,
        })
    }

    pub fn initialized(&self) -> bool {
        self.app.initialized()
    }

    pub fn set_board_relay(
        &self,
        boards: Vec<pepino_core::BoardId>,
        peer: pepino_app::PeerHint,
        carry: bool,
    ) -> Result<pepino_app::LocalState, CliError> {
        let local =
            self.runtime
                .block_on(self.app.set_board_relay(boards, peer, carry, self.now_ms))?;
        self.wrote.set(true);
        Ok(local)
    }

    pub fn forget_board(
        &self,
        board: pepino_core::BoardId,
    ) -> Result<pepino_app::LocalState, CliError> {
        Ok(self
            .runtime
            .block_on(self.app.forget_board(board, self.now_ms))?)
    }

    pub fn local_state(&self) -> &pepino_app::LocalState {
        self.app.local_state()
    }

    pub fn author_labels(&self, verbose: bool) -> impl Fn(Option<&str>) -> String + use<> {
        let local = self.app.local_state();
        let board = self
            .selected_board()
            .map_or(local.board_id(), |(board, _)| board.id);
        let devices = local.author_devices(board);
        let people = local.board_person_labels(board);
        move |id| author_label(id, &devices, &people, verbose)
    }

    pub fn selected_board(&self) -> Result<(&pepino_app::LocalBoard, &'static str), CliError> {
        let local = self.local_state();
        if let Some((reference, source)) = self.board_override.as_ref().map_err(Clone::clone)? {
            Ok((local.resolve_board(reference, source)?, source))
        } else {
            Ok((local.current_board(), "current"))
        }
    }

    pub fn call(&mut self, request: Request) -> Result<Response, CliError> {
        self.call_listing(request, false)
    }

    pub fn call_unscoped(&mut self, request: Request) -> Result<Response, CliError> {
        let write = matches!(&request, Request::Write(_))
            && !matches!(
                &request,
                Request::Write(pepino_app::WriteRequest::BoardUse { .. })
            );
        let response = self
            .runtime
            .block_on(self.app.handle(request, self.now_ms))?;
        if write {
            self.wrote.set(true);
        }
        Ok(response)
    }

    pub fn call_listing(
        &mut self,
        request: Request,
        all_boards: bool,
    ) -> Result<Response, CliError> {
        let write = matches!(&request, Request::Write(_));
        let identity_local = matches!(
            &request,
            Request::Read(ReadRequest::ListViews)
                | Request::Write(
                    pepino_app::WriteRequest::SaveView { .. }
                        | pepino_app::WriteRequest::DeleteView { .. }
                )
        );
        let board = if identity_local {
            None
        } else {
            self.board_override
                .as_ref()
                .map_err(Clone::clone)?
                .as_ref()
                .map(|(reference, source)| (reference.as_str(), *source))
        };
        if !identity_local && !all_boards && matches!(&request, Request::Read(_)) {
            let selected = self.selected_board()?.0;
            if self.local_state().board_removed(selected.id) {
                self.board_notice.replace(Some(crate::output::Warning {
                    message: format!(
                        "Board {:?} membership removed. Showing local data.",
                        selected.name
                    ),
                    details: serde_json::Map::from_iter([(
                        "membership".into(),
                        serde_json::json!("removed"),
                    )]),
                }));
            }
        }
        let response =
            self.runtime
                .block_on(self.app.handle_on(request, self.now_ms, board, all_boards))?;
        if write {
            self.wrote.set(true);
        }
        Ok(response)
    }

    pub fn task(&mut self, reference: &str) -> Result<TaskSummary, CliError> {
        let response = self.call(Request::Read(ReadRequest::GetTask {
            task: reference.into(),
        }))?;
        Ok(response.task()?.task.summary.clone())
    }

    pub fn check_membership_order(
        &mut self,
        task: &str,
        value: Option<&String>,
        usage: &str,
    ) -> Result<(), CliError> {
        let Some(reference) = value.filter(|value| {
            (4..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) else {
            return Ok(());
        };
        self.task(task)?;
        if self.task(reference).is_ok() {
            return Err(
                CliError::new(ErrorKind::Usage, format!("{reference} matches a task.")).with_hint(
                    format!("Use {usage} with one task first, followed by values."),
                ),
            );
        }
        Ok(())
    }

    pub fn resolve_tasks(&mut self, references: Vec<String>) -> Result<Vec<TaskSummary>, CliError> {
        references.iter().map(|task| self.task(task)).collect()
    }
}

#[cfg(test)]
#[path = "../../pepino-app/tests/support/template.rs"]
mod fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LoadedConfig, Platform, PlatformDirs};
    use crate::terminal::{Color, Terminal};
    use pepino_app::WriteRequest;

    use super::fixture;

    #[test]
    fn opens_the_configured_board_with_the_injected_clock() {
        let root = tempfile::tempdir().unwrap();
        let dirs = PlatformDirs::new(Platform::Linux, root.path(), None, None);
        let config = Config::defaults(&dirs).unwrap();
        let directory = config.data_dir.value.as_path().to_path_buf();
        let context = Context::new(
            Terminal::detect(Color::Never, &Default::default()),
            false,
            None,
            InvocationTime {
                now_ms: 1_795_149_900_000,
                today: "2026-11-19".parse().unwrap(),
                offset: chrono::FixedOffset::east_opt(0).unwrap(),
            },
            || {
                Ok(LoadedConfig {
                    config: config.clone(),
                    files: vec![],
                })
            },
        );
        fixture::prepare_blocking(&directory);
        let mut client = AppClient::open(&context).unwrap();
        assert_eq!(client.now_ms, context.time.now_ms);
        let Response::Status {
            data_dir, counts, ..
        } = client.call(Request::Read(ReadRequest::Status)).unwrap()
        else {
            panic!("expected status");
        };
        assert_eq!(std::path::Path::new(&data_dir), directory);
        assert_eq!(counts.open, 0);

        let created = client
            .call(Request::Write(crate::write_args::new_task(
                "Inspect greenhouse hinges".into(),
            )))
            .unwrap();
        let id = created.write().unwrap().created.as_ref().unwrap();
        let prefix = id[..8].to_uppercase();
        let resolved = client.resolve_tasks(vec![prefix.clone()]).unwrap();
        assert_eq!(resolved[0].id, *id);
        let missing = client
            .resolve_tasks(vec![prefix.clone(), "deadbeef".into()])
            .unwrap_err();
        assert_eq!(missing.kind(), ErrorKind::NotFound);
        assert!(missing.to_string().contains("deadbeef"));
        client
            .call(Request::Write(WriteRequest::Delete { task: id.clone() }))
            .unwrap();
        let deleted = client.task(&prefix).unwrap_err();
        assert_eq!(
            deleted.to_string(),
            format!("Task {id} \"Inspect greenhouse hinges\" was deleted")
        );
    }
}

fn author_label(
    id: Option<&str>,
    devices: &std::collections::BTreeMap<pepino_core::DeviceId, (pepino_app::PersonKey, String)>,
    people: &std::collections::BTreeMap<pepino_app::PersonKey, String>,
    verbose: bool,
) -> String {
    use crate::render::{short_id, single_line};

    let Some(id) = id else {
        return "unknown person".into();
    };
    let Some((person, device)) = id.parse().ok().and_then(|id| devices.get(&id)) else {
        return format!("unknown device {}", short_id(id));
    };
    let person = people
        .get(person)
        .map(|name| single_line(name))
        .unwrap_or_else(|| "unknown person".into());
    if verbose {
        format!("{person} ({})", single_line(device))
    } else {
        person
    }
}

#[cfg(test)]
#[test]
fn labels_authors_as_the_identity_or_an_unknown_device() {
    let cobalt = pepino_core::DeviceId::from_bytes([19; 32]);
    let identity = pepino_core::IdentityId::from_bytes([20; 32]);
    let devices = [(cobalt, ((identity, cobalt), "Garden tablet".into()))].into();
    let people = [((identity, cobalt), "Ada Gardener".into())].into();
    for (verbose, expected) in [
        (false, "Ada Gardener"),
        (true, "Ada Gardener (Garden tablet)"),
    ] {
        assert_eq!(
            author_label(Some(&cobalt.to_string()), &devices, &people, verbose),
            expected
        );
    }
    let stranger = "9a".repeat(32);
    assert_eq!(
        author_label(Some(&stranger), &devices, &people, false),
        "unknown device 9a9a9a"
    );
}
