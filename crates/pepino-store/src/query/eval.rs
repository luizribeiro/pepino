use std::{borrow::Borrow, collections::BTreeMap, fmt, rc::Rc};

use pepino_core::{
    IdentityId, TaskId,
    filter::{Assignee, Clause, Filter, Term},
};

use crate::{BoardView, PrefixResolution, TaskRecord};

use super::{
    QueryError, QueryErrorKind,
    predicates::{compare, custom, fold, has, is, text_matches, utc_days},
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

/// The current identity and in-memory resolvers supplied by the caller.
pub struct Context<'a> {
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
    predicates: Vec<Check<'view>>,
    view_count: usize,
}

impl fmt::Debug for Prepared<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("term_count", &self.predicates.len())
            .finish_non_exhaustive()
    }
}

impl<'view> Prepared<'view> {
    /// Tests a task from the snapshot used to prepare the query.
    pub fn matches(&self, task: &TaskRecord) -> bool {
        all_match(&self.predicates, task, &mut vec![None; self.view_count])
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
        let predicates = prepare(view, self.filter, &self.context, &mut saved)?.value;
        Ok(Prepared {
            predicates,
            view_count: saved.len(),
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

type Predicate<'a> = Box<dyn Fn(&TaskRecord) -> bool + 'a>;

struct Check<'view> {
    predicate: CheckKind<'view>,
    negated: bool,
}

enum CheckKind<'view> {
    Task(Predicate<'view>),
    View(Rc<ViewChecks<'view>>),
}

struct ViewChecks<'view> {
    id: usize,
    checks: Vec<Check<'view>>,
}

fn all_match(checks: &[Check<'_>], task: &TaskRecord, results: &mut [Option<bool>]) -> bool {
    checks.iter().all(|check| {
        let matched = match &check.predicate {
            CheckKind::Task(predicate) => predicate(task),
            CheckKind::View(view) => match results[view.id] {
                Some(matched) => matched,
                None => {
                    let matched = all_match(&view.checks, task, results);
                    results[view.id] = Some(matched);
                    matched
                }
            },
        };
        matched != check.negated
    })
}

fn prepare<'a>(
    view: &'a BoardView,
    filter: &Filter,
    context: &Context<'_>,
    saved: &mut SavedViews<'_, Rc<ViewChecks<'a>>>,
) -> Result<Expanded<Vec<Check<'a>>>, QueryError> {
    let mut predicates = Vec::new();
    let mut has_boards = false;
    for term in canonical_terms(filter) {
        let predicate = match &term.clause {
            Clause::Board(_) => {
                has_boards = true;
                continue;
            }
            Clause::View(name) => {
                let nested = saved.expand(term, name, |saved, nested| {
                    let expanded = prepare(view, nested, context, saved)?;
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
                let Some(predicate) = prepare_clause(view, clause, context)
                    .map_err(|kind| saved.error(term, kind))?
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

fn prepare_clause<'a>(
    view: &'a BoardView,
    clause: &Clause,
    context: &Context<'_>,
) -> Result<Option<Predicate<'a>>, QueryErrorKind> {
    let predicate: Predicate<'a> = match clause.clone() {
        Clause::Text(text) => {
            if !view.comments_loaded() {
                return Err(QueryErrorKind::CommentsRequired);
            }
            let needle = fold(&text);
            Box::new(move |task| text_matches(view, task, &needle))
        }
        Clause::Is(values) => Box::new(move |task| values.iter().any(|value| is(task, *value))),
        Clause::Has(values) => Box::new(move |task| values.iter().any(|value| has(task, value))),
        Clause::Tag(values) => {
            Box::new(move |task| values.iter().any(|tag| task.tags.contains(tag)))
        }
        Clause::Status(values) => Box::new(move |task| values.contains(&task.status)),
        Clause::Assignee(values) => {
            let ids = values
                .iter()
                .map(|value| resolve_person(context, value))
                .collect::<Result<Vec<_>, _>>()?;
            Box::new(move |task| ids.iter().any(|id| task.assignees.contains(id)))
        }
        Clause::Parent(values) => relation(view, &values, |task, id| task.parent == Some(id))?,
        Clause::Blocks(values) => relation(view, &values, |task, id| task.blocks.contains(&id))?,
        Clause::BlockedBy(values) => {
            relation(view, &values, |task, id| task.blocked_by.contains(&id))?
        }
        Clause::Priority(values) => Box::new(move |task| compare(task.priority, &values)),
        Clause::Due(values) => Box::new(move |task| compare(task.due, &values)),
        Clause::Created(values) => Box::new(move |task| utc_days(task.created, &values)),
        Clause::Updated(values) => Box::new(move |task| utc_days(task.updated, &values)),
        Clause::Custom {
            field,
            alternatives,
        } => Box::new(move |task| {
            task.custom
                .get(&field)
                .is_some_and(|value| custom(value, &alternatives))
        }),
        Clause::Board(_) | Clause::View(_) => return Ok(None),
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

fn relation<'a>(
    view: &BoardView,
    values: &[String],
    contains: fn(&TaskRecord, TaskId) -> bool,
) -> Result<Predicate<'a>, QueryErrorKind> {
    let ids = resolve_prefixes(view, values)?;
    Ok(Box::new(move |task| {
        ids.iter().any(|&id| contains(task, id))
    }))
}

fn resolve_prefixes(view: &BoardView, prefixes: &[String]) -> Result<Vec<TaskId>, QueryErrorKind> {
    prefixes
        .iter()
        .map(|prefix| match view.resolve_prefix(prefix) {
            Ok(PrefixResolution::One(id)) => Ok(id),
            Ok(PrefixResolution::None) => Err(QueryErrorKind::UnknownTaskPrefix {
                prefix: prefix.clone(),
            }),
            Ok(PrefixResolution::Ambiguous { count, candidates }) => {
                Err(QueryErrorKind::AmbiguousTaskPrefix {
                    prefix: prefix.clone(),
                    count,
                    candidates,
                })
            }
            Err(_) => Err(QueryErrorKind::InvalidTaskPrefix {
                prefix: prefix.clone(),
            }),
        })
        .collect()
}
