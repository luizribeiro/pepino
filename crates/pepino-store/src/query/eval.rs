use pepino_core::{
    IdentityId,
    filter::{Clause, Filter},
};

use crate::{BoardView, TaskRecord};

use super::{QueryError, QueryErrorKind};

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
    _chain: &mut Vec<&'a str>,
) -> Result<Vec<Predicate<'a>>, QueryError> {
    let mut predicates = Vec::<Predicate<'a>>::new();
    for term in filter.terms() {
        let at_term = |kind| QueryError {
            term: term.to_string(),
            kind,
        };
        let Some(predicate) = prepare_clause(view, &term.clause, context).map_err(at_term)? else {
            continue;
        };
        if term.negated {
            predicates.push(Box::new(move |task| !predicate(task)));
        } else {
            predicates.push(predicate);
        }
    }
    Ok(predicates)
}

fn prepare_clause<'a>(
    _view: &'a BoardView,
    clause: &'a Clause,
    _context: &Context<'a>,
) -> Result<Option<Predicate<'a>>, QueryErrorKind> {
    let predicate: Predicate<'a> = match clause {
        Clause::Tag(values) => {
            Box::new(move |task| values.iter().any(|tag| task.tags.contains(tag)))
        }
        Clause::Status(values) => Box::new(move |task| values.contains(&task.status)),
        _ => return Ok(None),
    };
    Ok(Some(predicate))
}
