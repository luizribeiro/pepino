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
pub type PersonResolver<'a> = dyn Fn(&str) -> Vec<IdentityId> + 'a;

/// Looks up parsed saved filters by name, without the leading `@`.
pub trait ViewResolver {
    fn resolve(&self, name: &str) -> Option<&Filter>;
}

/// The current identity and in-memory resolvers supplied by the caller.
pub struct Context<'a> {
    pub me: Option<IdentityId>,
    pub people: &'a PersonResolver<'a>,
    pub views: &'a dyn ViewResolver,
}

/// A parsed filter and its evaluation context.
pub struct Query<'a> {
    pub filter: &'a Filter,
    pub context: Context<'a>,
}

/// Evaluates the query for a task from the supplied board snapshot.
pub fn matches(view: &BoardView, task: &TaskRecord, query: &Query<'_>) -> Result<bool, QueryError> {
    let predicates = prepare(view, query.filter, &query.context, &mut Vec::new())?;
    Ok(all_match(&predicates, task))
}

/// Returns matching tasks in the order of the board snapshot.
pub fn select<'view>(
    view: &'view BoardView,
    query: &Query<'_>,
) -> Result<Vec<&'view TaskRecord>, QueryError> {
    let predicates = prepare(view, query.filter, &query.context, &mut Vec::new())?;
    Ok(view
        .iter()
        .filter(|task| all_match(&predicates, task))
        .collect())
}

type Predicate<'a> = Box<dyn Fn(&TaskRecord) -> bool + 'a>;

fn all_match(predicates: &[Predicate<'_>], task: &TaskRecord) -> bool {
    predicates.iter().all(|predicate| predicate(task))
}

fn prepare<'a>(
    view: &'a BoardView,
    filter: &'a Filter,
    context: &Context<'a>,
    chain: &mut Vec<&'a str>,
) -> Result<Vec<Predicate<'a>>, QueryError> {
    let mut predicates = Vec::<Predicate<'a>>::new();
    for term in filter.terms() {
        let at_term = |kind| QueryError {
            term: term.to_string(),
            kind,
        };
        let predicate = match &term.clause {
            Clause::Board(_) => continue,
            Clause::View(name) => {
                let nested = resolve_view(context, chain, name).map_err(at_term)?;
                chain.push(name);
                let nested = prepare(view, nested, context, chain)?;
                chain.pop();
                Box::new(move |task: &TaskRecord| all_match(&nested, task)) as Predicate<'a>
            }
            clause => prepare_clause(view, clause, context).map_err(at_term)?,
        };
        if term.negated {
            predicates.push(Box::new(move |task| !predicate(task)));
        } else {
            predicates.push(predicate);
        }
    }
    Ok(predicates)
}

fn resolve_view<'a>(
    context: &Context<'a>,
    chain: &[&str],
    name: &str,
) -> Result<&'a Filter, QueryErrorKind> {
    if chain.contains(&name) {
        return Err(QueryErrorKind::ViewCycle {
            chain: chain
                .iter()
                .copied()
                .chain([name])
                .map(str::to_owned)
                .collect(),
        });
    }
    context
        .views
        .resolve(name)
        .ok_or_else(|| QueryErrorKind::UnknownView { name: name.into() })
}

fn prepare_clause<'a>(
    view: &'a BoardView,
    clause: &'a Clause,
    context: &Context<'a>,
) -> Result<Predicate<'a>, QueryErrorKind> {
    let predicate: Predicate<'a> = match clause {
        Clause::Text(text) => {
            let needle = fold(text);
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
        Clause::Parent(values) => {
            let ids = resolve_prefixes(view, values)?;
            Box::new(move |task| task.parent.is_some_and(|parent| ids.contains(&parent)))
        }
        Clause::Blocks(values) => {
            let ids = resolve_prefixes(view, values)?;
            Box::new(move |task| ids.iter().any(|id| task.blocks.contains(id)))
        }
        Clause::BlockedBy(values) => {
            let ids = resolve_prefixes(view, values)?;
            Box::new(move |task| ids.iter().any(|id| task.blocked_by.contains(id)))
        }
        Clause::Priority(values) => Box::new(move |task| compare(task.priority, values)),
        Clause::Due(values) => Box::new(move |task| compare(task.due, values)),
        Clause::Created(values) => Box::new(move |task| utc_days(task.created, values)),
        Clause::Updated(values) => Box::new(move |task| utc_days(task.updated, values)),
        Clause::Custom {
            field,
            alternatives,
        } => Box::new(move |task| {
            task.custom
                .get(field)
                .is_some_and(|value| custom(value, alternatives))
        }),
        Clause::Board(_) | Clause::View(_) => unreachable!("handled during filter preparation"),
    };
    Ok(predicate)
}

fn resolve_person(
    context: &Context<'_>,
    assignee: &Assignee,
) -> Result<IdentityId, QueryErrorKind> {
    let Assignee::Person(text) = assignee else {
        return context.me.ok_or(QueryErrorKind::MissingIdentity);
    };
    let mut candidates = (context.people)(text);
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
