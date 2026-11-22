use pepino_app::{Response, SavedView};
use ppn::error::{CliError, ErrorKind};
use ppn::output::Render;
use ppn::terminal::Terminal;
use serde_json::{Map, Value, json};

pub(super) struct ViewDefinition(pub SavedView);

impl Render for ViewDefinition {
    fn human(&self, _: &Terminal) -> String {
        definition(&self.0)
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), "view".into()),
            (
                "view".into(),
                json!({"name": self.0.name, "filter": self.0.filter}),
            ),
        ])
    }
}

pub(super) fn saved_views(response: &Response, color: bool) -> Result<String, CliError> {
    match response {
        Response::Views { views, .. } => Ok(views
            .iter()
            .map(|view| list_row(view, color))
            .collect::<Vec<_>>()
            .join("\n")),
        Response::ViewSaved { view, replaced } => {
            let verb = if *replaced { "Replaced" } else { "Saved" };
            Ok(format!("{verb} {}", view.name))
        }
        Response::ViewDeleted { view } => {
            Ok(format!("Removed {}", ppn::render::single_line(&view.name)))
        }
        _ => Err(CliError::new(
            ErrorKind::Failure,
            "Expected a saved view response.",
        )),
    }
}

fn list_row(view: &SavedView, color: bool) -> String {
    let mut row = if let Some(label) = &view.recovered_as {
        format!("{label} [@{}]  {}", view.name, view.filter)
    } else {
        definition(view)
    };
    if let (Some(by), Some(at)) = (&view.updated_by, &view.updated_at) {
        let suffix = format!("  (updated by {} at {at})", by.name);
        if color {
            row.push_str("\x1b[2m");
            row.push_str(&suffix);
            row.push_str("\x1b[0m");
        } else {
            row.push_str(&suffix);
        }
    }
    row
}

fn definition(view: &SavedView) -> String {
    format!("{}  {}", ppn::render::single_line(&view.name), view.filter)
}

#[cfg(test)]
mod tests {
    use super::{ViewDefinition, saved_views};
    use pepino_app::Response;
    use ppn::output::Render;
    use serde_json::json;

    #[test]
    fn renders_canonical_definitions_and_replacement_outcomes() {
        let saved = json!({"name":"weekend","filter":"tag:hardware due:<fri"});
        let cases = [
            (
                json!({"kind":"views","views":[saved]}),
                "weekend  tag:hardware due:<fri",
            ),
            (
                json!({"kind":"view_saved","view":saved,"replaced":true}),
                "Replaced weekend",
            ),
            (
                json!({"kind":"view_saved","view":saved,"replaced":false}),
                "Saved weekend",
            ),
            (
                json!({"kind":"view_deleted","view":saved}),
                "Removed weekend",
            ),
        ];
        let view = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(
            serde_json::Value::Object(ViewDefinition(view).json_fields()),
            json!({"kind":"view","view":saved})
        );
        for (response, expected) in cases {
            let response: Response = serde_json::from_value(response).unwrap();
            assert_eq!(saved_views(&response, false).unwrap(), expected);
        }
    }
}
