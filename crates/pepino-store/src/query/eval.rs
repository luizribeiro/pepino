use pepino_core::{IdentityId, filter::Filter};

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
