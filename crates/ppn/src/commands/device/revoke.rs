use chrono::Utc;
use pepino_sync::{BoardRemoval, Removal};
use ppn::{
    app::AppClient,
    daemon::{address::socket_path, client::Client},
    render::short_id,
};

use super::*;

pub(super) fn run(context: &Context<'_>, reference: &str) -> Result<CommandResult, CliError> {
    let client = AppClient::open(context)?;
    let local = client.local_state();
    local.check_device_write()?;
    let target = local.revocation_target(reference)?;
    let name = single_line(&target.name);
    let prompt = format!("Revoke {name} ({})?", short_id(&target.id.to_string()));
    let mut result = Report {
        device: target.id,
        name,
        confirmed: false,
        boards: Vec::new(),
    };
    if !context.confirm(&prompt)? {
        return Ok(CommandResult::document(result));
    }
    let (directory, options) = local_options(context, None)?;
    let local = runtime()?.block_on(async {
        let app = App::open_with_options(directory, context.time.now_ms, options).await?;
        app.record_device_revocation(target.id, Utc::now()).await
    })?;
    let daemon = Client {
        path: socket_path(&context.config()?.config),
    };
    let _ = daemon.nudge();
    result.confirmed = true;
    result.boards = local
        .boards()
        .iter()
        .cloned()
        .map(|board| BoardRemoval {
            board,
            result: Removal::Unavailable,
        })
        .collect();
    Ok(CommandResult::document(result))
}

struct Report {
    device: DeviceId,
    name: String,
    confirmed: bool,
    boards: Vec<BoardRemoval>,
}

impl Render for Report {
    fn human(&self, _: &Terminal) -> String {
        if self.confirmed {
            format!(
                "Revoked {} ({}).",
                self.name,
                short_id(&self.device.to_string())
            )
        } else {
            "Revocation cancelled.".into()
        }
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("device_revoke")),
            ("device_id".into(), json!(self.device)),
            ("name".into(), json!(self.name)),
            ("confirmed".into(), json!(self.confirmed)),
            ("boards".into(), json!(self.boards)),
        ])
    }
}
