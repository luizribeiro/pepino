use chrono::Utc;
use ppn::{
    app::AppClient,
    daemon::{address::socket_path, client::Client},
};

use super::*;
use crate::commands::daemon::ensure_running;

pub(super) fn run(
    context: &Context<'_>,
    selection: Option<&Path>,
    allow_start: bool,
) -> Result<CommandResult, CliError> {
    let app = AppClient::open(context)?;
    let local = app.local_state();
    local.check_can_leave()?;
    let config = &context.config()?.config;
    let mut report = Leave {
        device: local.device_id,
        requested_at: None,
        daemon_notified: false,
        data: config.data_dir.value.as_path().to_owned(),
        credentials: config.credentials_file.value.as_path().to_owned(),
    };
    let prompt = format!(
        "Retire {} from {}? This device will stop syncing tasks. Another active device must complete the removal. Previously synced data will stay on disk.",
        single_line(local.device_name()),
        single_line(local.display_name()),
    );
    if context.confirm(&prompt)? {
        let (directory, options) = local_options(context, None)?;
        let local = runtime()?.block_on(async {
            let app = App::open_with_options(directory, context.time.now_ms, options).await?;
            app.record_leave_request(Utc::now()).await
        })?;
        report.requested_at = local.device_leave_requested_at();
        let daemon = Client {
            path: socket_path(config),
        };
        report.daemon_notified = daemon.nudge().is_ok()
            || (allow_start
                && config.daemon.autostart.value
                && ensure_running(&daemon, context, selection, true).is_ok()
                && daemon.nudge().is_ok());
    }
    Ok(CommandResult::document(report))
}

struct Leave {
    device: DeviceId,
    requested_at: Option<chrono::DateTime<Utc>>,
    daemon_notified: bool,
    data: std::path::PathBuf,
    credentials: std::path::PathBuf,
}

impl Render for Leave {
    fn human(&self, _: &Terminal) -> String {
        if self.requested_at.is_none() {
            return "Leave cancelled.".into();
        }
        let publication = if self.daemon_notified {
            "The daemon will publish the request."
        } else {
            "Start the daemon to publish the request."
        };
        format!(
            "Leave requested. {publication} Another active device must complete the removal. Task sync is paused on this device.\nExisting tasks, the identity record, and the local keys remain on disk. Check `ppn status` to see when the request is complete. To wipe this device, stop the daemon and delete the data directory ({}) and credentials file ({}).",
            single_line(&self.data.display().to_string()),
            single_line(&self.credentials.display().to_string()),
        )
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("device_leave")),
            ("device_id".into(), json!(self.device)),
            ("requested_at".into(), json!(self.requested_at)),
            ("pending".into(), json!(self.requested_at.is_some())),
            ("daemon_notified".into(), json!(self.daemon_notified)),
            ("data_dir".into(), json!(self.data)),
            ("credentials_file".into(), json!(self.credentials)),
        ])
    }
}
