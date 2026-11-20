use std::{borrow::Borrow, collections::BTreeMap, fmt};

use pepino_core::{
    IdentityId, TaskId,
    filter::{Assignee, Clause, Filter},
};

use crate::{BoardView, PrefixResolution, TaskRecord};

use super::{
    QueryError, QueryErrorKind,
    predicates::{compare, custom, fold, has, is, text_matches, utc_days},
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
        all_match(&self.predicates, task)
    }
}

impl Query<'_> {
    /// Resolves the filter before evaluating any task.
    pub fn prepare<'view>(&self, view: &'view BoardView) -> Result<Prepared<'view>, QueryError> {
        let predicates = prepare(view, self.filter, &self.context, &mut Vec::new())?;
        Ok(Prepared { predicates })
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
    predicate: Predicate<'view>,
    negated: bool,
}

fn all_match(checks: &[Check<'_>], task: &TaskRecord) -> bool {
    checks
        .iter()
        .all(|check| (check.predicate)(task) != check.negated)
}

fn prepare<'a>(
    view: &'a BoardView,
    filter: &Filter,
    context: &Context<'_>,
    chain: &mut Vec<String>,
) -> Result<Vec<Check<'a>>, QueryError> {
    let mut predicates = Vec::new();
    let mut terms: Vec<_> = filter.terms().iter().collect();
    terms.sort_by_cached_key(|term| term.to_string());
    for term in terms {
        let at_term = |kind| QueryError {
            term: term.to_string(),
            kind,
            view_chain: chain.clone(),
        };
        let predicate = match &term.clause {
            Clause::Board(_) => continue,
            Clause::View(name) => {
                let nested = resolve_view(context, chain, name).map_err(at_term)?;
                chain.push(name.clone());
                let nested = prepare(view, &nested, context, chain)?;
                chain.pop();
                Box::new(move |task: &TaskRecord| all_match(&nested, task)) as Predicate<'a>
            }
            clause => {
                let Some(predicate) = prepare_clause(view, clause, context).map_err(at_term)?
                else {
                    continue;
                };
                predicate
            }
        };
        predicates.push(Check {
            predicate,
            negated: term.negated,
        });
    }
    Ok(predicates)
}

fn resolve_view(
    context: &Context<'_>,
    chain: &[String],
    name: &str,
) -> Result<Filter, QueryErrorKind> {
    if chain.iter().any(|active| active == name) {
        return Err(QueryErrorKind::ViewCycle {
            chain: chain.iter().cloned().chain([name.to_owned()]).collect(),
        });
    }
    context
        .views
        .resolve(name)
        .ok_or_else(|| QueryErrorKind::UnknownView { name: name.into() })
}

fn prepare_clause<'a>(
    view: &'a BoardView,
    clause: &Clause,
    context: &Context<'_>,
) -> Result<Option<Predicate<'a>>, QueryErrorKind> {
    let predicate: Predicate<'a> = match clause.clone() {
        Clause::Text(text) => {
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
