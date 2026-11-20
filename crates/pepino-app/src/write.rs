use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{Event, FieldName, IdentityId, InitialFields, TaskId};
use pepino_store::BoardView;

use crate::app::invalid;
use crate::batch::Batch;
use crate::{App, AppError, EdgeKind, FieldValue, Request};

impl App {
    pub(super) async fn plan_write(
        &self,
        view: &BoardView,
        batch: &mut Batch,
        request: Request,
    ) -> Result<(), AppError> {
        match request {
            Request::AddTask {
                title,
                description,
                status,
                due,
                priority,
                tags,
                assignees,
                fields,
                parent,
                blocks,
                blocked_by,
            } => {
                let fields = InitialFields {
                    description,
                    status,
                    due,
                    priority,
                    tags: tags.into_iter().collect(),
                    assignees: self.identities(assignees)?,
                    custom: custom_fields(fields)?,
                };
                let created = TaskId(batch.push(Event::TaskCreated { title, fields }, "fields")?);
                if let Some(parent) = parent {
                    let to = self.resolve(view, "parent", &parent).await?.id;
                    batch.push(
                        Event::EdgeAdded {
                            from: created,
                            kind: EdgeKind::Parent,
                            to,
                        },
                        "parent",
                    )?;
                }
                for (field, references) in [("blocks", blocks), ("blocked_by", blocked_by)] {
                    let mut ids = BTreeSet::new();
                    for reference in references {
                        ids.insert(self.resolve(view, field, &reference).await?.id);
                    }
                    for target in ids {
                        let (from, to) = if field == "blocks" {
                            (created, target)
                        } else {
                            (target, created)
                        };
                        batch.push(
                            Event::EdgeAdded {
                                from,
                                kind: EdgeKind::Blocks,
                                to,
                            },
                            field,
                        )?;
                    }
                }
            }
            _ => return Err(invalid("op", "write operation is unavailable")),
        }
        Ok(())
    }

    fn identities(&self, references: Vec<String>) -> Result<BTreeSet<IdentityId>, AppError> {
        references
            .into_iter()
            .map(|reference| {
                if reference == "me" {
                    Ok(self.local.identity_id)
                } else {
                    reference.parse().map_err(|_| {
                        invalid(
                            "assignees",
                            format_args!(
                                "{reference:?} must be a full hexadecimal identity ID or me"
                            ),
                        )
                    })
                }
            })
            .collect()
    }
}

fn custom_fields(
    fields: BTreeMap<String, FieldValue>,
) -> Result<BTreeMap<FieldName, FieldValue>, AppError> {
    fields
        .into_iter()
        .map(|(name, value)| {
            let field =
                FieldName::new(&name).map_err(|error| invalid(format!("fields.{name}"), error))?;
            if !field.is_custom() {
                return Err(invalid(
                    format!("fields.{name}"),
                    "expected a custom field name",
                ));
            }
            Ok((field, value))
        })
        .collect()
}
