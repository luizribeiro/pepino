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
    runtime: Runtime,
    now_ms: u64,
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
            wrote: context.wrote.clone(),
            runtime,
            now_ms,
        })
    }

    pub fn author_labels(&self, verbose: bool) -> impl Fn(Option<&str>) -> String + use<> {
        let local = self.app.local_state();
        let local_id = local.device_id.to_string();
        let local_person = crate::render::single_line(&local.display_name);
        let local_device = crate::render::single_line(&local.device_name);
        move |id| {
            let Some(id) = id else {
                return "unknown person".into();
            };
            let (person, device) = if id == local_id {
                (local_person.as_str(), local_device.as_str())
            } else {
                ("unknown person", crate::render::short_id(id))
            };
            if verbose {
                format!("{person} ({device})")
            } else {
                person.into()
            }
        }
    }

    pub fn call(&mut self, request: Request) -> Result<Response, CliError> {
        let write = matches!(&request, Request::Write(_));
        let response = self
            .runtime
            .block_on(self.app.handle(request, self.now_ms))?;
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
        fixture::copy(&directory);
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
