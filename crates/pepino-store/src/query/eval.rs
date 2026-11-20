use std::{borrow::Borrow, collections::BTreeMap, fmt, rc::Rc};

use pepino_core::{
    Date, IdentityId, TaskId,
    filter::{Assignee, Clause, Comparison, DateOperand, Filter, Term},
};

use crate::{BoardView, PrefixResolution, TaskRecord};

use super::{
    QueryError, QueryErrorKind,
    predicates::{compare, custom, fold, folded_text, has, is, utc_days},
    saved::{self, Expanded, SavedViews, canonical_terms},
};

/// Resolves decoded person text to candidate identities.
pub trait PersonResolver {
    fn resolve(&self, text: &str) -> Vec<IdentityId>;
}

impl<F: Fn(&str) -> Vec<IdentityId>> PersonResolver for F {
    fn resolve(&self, text: &str) -> Vec<IdentityId> {
        self(text)
    }
}

impl<K: Borrow<str> + Ord> PersonResolver for BTreeMap<K, Vec<IdentityId>> {
    fn resolve(&self, text: &str) -> Vec<IdentityId> {
        self.get(text).cloned().unwrap_or_default()
    }
}

/// Looks up parsed saved filters by name, without the leading `@`.
pub trait ViewResolver {
    fn resolve(&self, name: &str) -> Option<Filter>;
}

impl<F: Fn(&str) -> Option<Filter>> ViewResolver for F {
    fn resolve(&self, name: &str) -> Option<Filter> {
        self(name)
    }
}

impl<K: Borrow<str> + Ord> ViewResolver for BTreeMap<K, Filter> {
    fn resolve(&self, name: &str) -> Option<Filter> {
        self.get(name).cloned()
    }
}

/// The calendar date, current identity and in-memory resolvers supplied by the caller.
pub struct Context<'a> {
    pub today: Date,
    pub me: Option<IdentityId>,
    pub people: &'a dyn PersonResolver,
    pub views: &'a dyn ViewResolver,
}

/// A parsed filter and its evaluation context.
pub struct Query<'a> {
    pub filter: &'a Filter,
    pub context: Context<'a>,
}

/// A query resolved against a particular board snapshot.
pub struct Prepared<'view> {
    predicates: Vec<Check>,
    view_count: usize,
    view: &'view BoardView,
    resolved_dates: BTreeMap<String, Date>,
}

impl fmt::Debug for Prepared<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("term_count", &self.predicates.len())
            .finish_non_exhaustive()
    }
}

impl<'view> Prepared<'view> {
    /// Calendar dates used for relative operands, keyed by canonical spelling.
    pub fn resolved_dates(&self) -> &BTreeMap<String, Date> {
        &self.resolved_dates
    }

    /// Tests a task from the snapshot used to prepare the query.
    pub fn matches(&self, task: &TaskRecord) -> bool {
        let mut evaluation = TaskEvaluation {
            task,
            view: self.view,
            views: vec![None; self.view_count],
            folded: None,
        };
        evaluation.matches(&self.predicates)
    }
}

impl Query<'_> {
    /// Extracts board terms from this filter and its reachable saved views.
    ///
    /// Terms retain their alternatives and negation. Each saved definition contributes
    /// its terms once. Negating a saved view that selects boards has no board-selection
    /// meaning and returns an error, including when the board terms are in a nested view.
    pub fn board_terms(&self) -> Result<Vec<Term>, QueryError> {
        saved::board_terms(self.filter, self.context.views)
    }
    /// Resolves the filter before evaluating any task.
    pub fn prepare<'view>(&self, view: &'view BoardView) -> Result<Prepared<'view>, QueryError> {
        let mut saved = SavedViews::new(self.context.views);
        let mut operands = Operands::new(view, &self.context);
        let predicates = prepare(self.filter, &mut operands, &mut saved)?.value;
        Ok(Prepared {
            predicates,
            view_count: saved.len(),
            view,
            resolved_dates: operands.dates,
        })
    }
}

/// Evaluates the query for a task from the supplied board snapshot.
pub fn matches(view: &BoardView, task: &TaskRecord, query: &Query<'_>) -> Result<bool, QueryError> {
    Ok(query.prepare(view)?.matches(task))
}

/// Returns matching tasks in the order of the board snapshot.
pub fn select<'view>(
    view: &'view BoardView,
    query: &Query<'_>,
) -> Result<Vec<&'view TaskRecord>, QueryError> {
    let prepared = query.prepare(view)?;
    Ok(view.iter().filter(|task| prepared.matches(task)).collect())
}

type Predicate = Box<dyn Fn(&TaskRecord) -> bool>;

struct Check {
    predicate: CheckKind,
    negated: bool,
}

enum CheckKind {
    Task(Predicate),
    Text(String),
    View(Rc<ViewChecks>),
}

struct ViewChecks {
    id: usize,
    checks: Vec<Check>,
}

struct TaskEvaluation<'a> {
    task: &'a TaskRecord,
    view: &'a BoardView,
    views: Vec<Option<bool>>,
    folded: Option<Vec<String>>,
}

impl TaskEvaluation<'_> {
    fn matches(&mut self, checks: &[Check]) -> bool {
        checks.iter().all(|check| {
            let matched = match &check.predicate {
                CheckKind::Task(predicate) => predicate(self.task),
                CheckKind::Text(needle) => self
                    .folded
                    .get_or_insert_with(|| folded_text(self.view, self.task))
                    .iter()
                    .any(|part| part.contains(needle)),
                CheckKind::View(view) => match self.views[view.id] {
                    Some(matched) => matched,
                    None => {
                        let matched = self.matches(&view.checks);
                        self.views[view.id] = Some(matched);
                        matched
                    }
                },
            };
            matched != check.negated
        })
    }
}

fn prepare(
    filter: &Filter,
    operands: &mut Operands<'_>,
    saved: &mut SavedViews<'_, Rc<ViewChecks>>,
) -> Result<Expanded<Vec<Check>>, QueryError> {
    let mut predicates = Vec::new();
    let mut has_boards = false;
    for indexed @ (_, term) in canonical_terms(filter) {
        let predicate = match &term.clause {
            Clause::Board(_) => {
                has_boards = true;
                continue;
            }
            Clause::Text(text) => {
                if !operands.view.comments_loaded() {
                    return Err(saved.error(indexed, QueryErrorKind::CommentsRequired));
                }
                CheckKind::Text(fold(text))
            }
            Clause::View(name) => {
                let nested = saved.expand(indexed, name, |saved, nested| {
                    let expanded = prepare(nested, operands, saved)?;
                    Ok(Expanded {
                        value: Rc::new(ViewChecks {
                            id: saved.len(),
                            checks: expanded.value,
                        }),
                        has_boards: expanded.has_boards,
                    })
                })?;
                has_boards |= nested.has_boards;
                CheckKind::View(nested.value)
            }
            clause => {
                let Some(predicate) =
                    prepare_clause(clause, operands).map_err(|kind| saved.error(indexed, kind))?
                else {
                    continue;
                };
                CheckKind::Task(predicate)
            }
        };
        predicates.push(Check {
            predicate,
            negated: term.negated,
        });
    }
    Ok(Expanded {
        value: predicates,
        has_boards,
    })
}

fn prepare_clause(
    clause: &Clause,
    operands: &mut Operands<'_>,
) -> Result<Option<Predicate>, QueryErrorKind> {
    let predicate: Predicate = match clause.clone() {
        Clause::Is(values) => Box::new(move |task| values.iter().any(|value| is(task, *value))),
        Clause::Has(values) => Box::new(move |task| values.iter().any(|value| has(task, value))),
        Clause::Tag(values) => {
            Box::new(move |task| values.iter().any(|tag| task.tags.contains(tag)))
        }
        Clause::Status(values) => Box::new(move |task| values.contains(&task.status)),
        Clause::Assignee(values) => {
            let ids = values
                .iter()
                .map(|value| operands.person(value))
                .collect::<Result<Vec<_>, _>>()?;
            Box::new(move |task| ids.iter().any(|id| task.assignees.contains(id)))
        }
        Clause::Parent(values) => relation(operands, &values, |task, id| task.parent == Some(id))?,
        Clause::Blocks(values) => {
            relation(operands, &values, |task, id| task.blocks.contains(&id))?
        }
        Clause::BlockedBy(values) => {
            relation(operands, &values, |task, id| task.blocked_by.contains(&id))?
        }
        Clause::Priority(values) => Box::new(move |task| compare(task.priority, &values)),
        Clause::Due(values) => {
            let values = operands.dates(values)?;
            Box::new(move |task| compare(task.due, &values))
        }
        Clause::Created(values) => {
            let values = operands.dates(values)?;
            Box::new(move |task| utc_days(task.created, &values))
        }
        Clause::Updated(values) => {
            let values = operands.dates(values)?;
            Box::new(move |task| utc_days(task.updated, &values))
        }
        Clause::Custom {
            field,
            alternatives,
        } => Box::new(move |task| {
            task.custom
                .get(&field)
                .is_some_and(|value| custom(value, &alternatives))
        }),
        Clause::Board(_) | Clause::View(_) | Clause::Text(_) => return Ok(None),
    };
    Ok(Some(predicate))
}

fn resolve_person(
    context: &Context<'_>,
    assignee: &Assignee,
) -> Result<IdentityId, QueryErrorKind> {
    let Assignee::Person(text) = assignee else {
        return context.me.ok_or(QueryErrorKind::MissingIdentity);
    };
    let mut candidates = context.people.resolve(text);
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [] => Err(QueryErrorKind::UnknownPerson { text: text.clone() }),
        [person] => Ok(*person),
        _ => Err(QueryErrorKind::AmbiguousPerson {
            text: text.clone(),
            candidates,
        }),
    }
}

fn relation(
    operands: &mut Operands<'_>,
    values: &[String],
    contains: fn(&TaskRecord, TaskId) -> bool,
) -> Result<Predicate, QueryErrorKind> {
    let ids = values
        .iter()
        .map(|prefix| operands.prefix(prefix))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Box::new(move |task| {
        ids.iter().any(|&id| contains(task, id))
    }))
}

struct Operands<'a> {
    view: &'a BoardView,
    context: &'a Context<'a>,
    people: BTreeMap<String, IdentityId>,
    prefixes: BTreeMap<String, TaskId>,
    dates: BTreeMap<String, Date>,
}

impl<'a> Operands<'a> {
    fn new(view: &'a BoardView, context: &'a Context<'a>) -> Self {
        Self {
            view,
            context,
            people: BTreeMap::new(),
            prefixes: BTreeMap::new(),
            dates: BTreeMap::new(),
        }
    }

    fn dates(
        &mut self,
        values: Vec<Comparison<DateOperand>>,
    ) -> Result<Vec<Comparison<Date>>, QueryErrorKind> {
        values
            .into_iter()
            .map(|Comparison { operator, value }| {
                let value = self.date(&value)?;
                Ok(Comparison { operator, value })
            })
            .collect()
    }

    fn date(&mut self, operand: &DateOperand) -> Result<Date, QueryErrorKind> {
        if let DateOperand::Absolute(date) = operand {
            return Ok(*date);
        }
        let expression = operand.to_string();
        if let Some(&date) = self.dates.get(&expression) {
            return Ok(date);
        }
        let date =
            operand
                .resolve(self.context.today)
                .map_err(|_| QueryErrorKind::DateOutOfRange {
                    expression: expression.clone(),
                })?;
        self.dates.insert(expression, date);
        Ok(date)
    }

    fn person(&mut self, assignee: &Assignee) -> Result<IdentityId, QueryErrorKind> {
        if let Assignee::Person(text) = assignee {
            if let Some(&id) = self.people.get(text) {
                return Ok(id);
            }
            let id = resolve_person(self.context, assignee)?;
            self.people.insert(text.clone(), id);
            Ok(id)
        } else {
            resolve_person(self.context, assignee)
        }
    }

    fn prefix(&mut self, prefix: &str) -> Result<TaskId, QueryErrorKind> {
        let key = prefix.to_ascii_lowercase();
        if let Some(&id) = self.prefixes.get(&key) {
            return Ok(id);
        }
        let id = match self.view.resolve_prefix(prefix) {
            Ok(PrefixResolution::One(id)) => Ok(id),
            Ok(PrefixResolution::None) => Err(QueryErrorKind::UnknownTaskPrefix {
                prefix: prefix.into(),
            }),
            Ok(PrefixResolution::Ambiguous { count, candidates }) => {
                Err(QueryErrorKind::AmbiguousTaskPrefix {
                    prefix: prefix.into(),
                    count,
                    candidates,
                })
            }
            Err(_) => Err(QueryErrorKind::InvalidTaskPrefix {
                prefix: prefix.into(),
            }),
        }?;
        self.prefixes.insert(key, id);
        Ok(id)
    }
}
