use proptest::{collection::vec, prelude::*, sample::select};

use crate::FieldName;

use super::keywords::STATUSES;
use super::*;

fn atom() -> impl Strategy<Value = String> {
    let punctuation = select(vec![
        ' ', '\t', '\n', '\u{2003}', '"', '\\', ':', ',', '<', '>', '=', '-', '@',
    ]);
    vec(prop_oneof![1 => any::<char>(), 3 => punctuation], 1..16)
        .prop_map(|chars| chars.into_iter().collect())
}

fn operator() -> impl Strategy<Value = Operator> {
    use Operator::*;
    select(vec![Eq, Lt, Le, Gt, Ge])
}

fn matching(
    key: impl Strategy<Value = String>,
    operand: impl Strategy<Value = (Operator, Value)>,
) -> impl Strategy<Value = Clause> {
    (key, vec(operand, 1..4)).prop_map(|(key, operands)| Clause::Match {
        key,
        alternatives: operands
            .into_iter()
            .map(|(operator, value)| Comparison { operator, value })
            .collect(),
    })
}

fn named(names: Vec<&'static str>) -> impl Strategy<Value = String> {
    select(names).prop_map(str::to_owned)
}

fn clause() -> impl Strategy<Value = Clause> {
    let states = select(
        State::WORDS
            .iter()
            .map(|(state, _)| *state)
            .collect::<Vec<_>>(),
    );
    let presence = prop_oneof![
        select(
            PresenceField::WORDS
                .iter()
                .map(|(field, _)| field.clone())
                .collect::<Vec<_>>()
        ),
        "custom_[a-z0-9_-]{0,56}"
            .prop_map(|name| PresenceField::Custom(FieldName::new(name).unwrap())),
    ];
    let custom_key = prop_oneof![Just("priority".into()), "custom_[a-z0-9_-]{0,56}",];
    let dates = prop_oneof![
        select(vec![Date::MIN, Date::MAX]),
        (-200_000..=200_000, 1_u32..=12, 1_u32..=28)
            .prop_map(|(y, m, d)| Date::from_ymd_opt(y, m, d).unwrap()),
    ];
    prop_oneof![
        atom().prop_map(Clause::Text),
        "[a-z][a-z0-9_-]{0,63}".prop_map(Clause::View),
        vec(states, 1..4).prop_map(Clause::Is),
        vec(presence, 1..4).prop_map(Clause::Has),
        matching(custom_key, (operator(), atom().prop_map(Value::Text))),
        matching(
            named(vec!["due", "created", "updated"]),
            (operator(), dates.prop_map(Value::Date))
        ),
        vec("[a-z][a-z0-9_./-]{0,16}", 1..4).prop_map(Clause::Tag),
        vec(
            select(
                STATUSES
                    .iter()
                    .map(|(status, _)| *status)
                    .collect::<Vec<_>>()
            ),
            1..4
        )
        .prop_map(Clause::Status),
        vec(
            prop_oneof![
                Just(Assignee::Me),
                Just(Assignee::Person("me".into())),
                atom().prop_map(Assignee::Person)
            ],
            1..4
        )
        .prop_map(Clause::Assignee),
        vec(atom(), 1..4).prop_map(Clause::Board),
        vec(atom(), 1..4).prop_map(Clause::Parent),
        vec(atom(), 1..4).prop_map(Clause::Blocks),
        vec(atom(), 1..4).prop_map(Clause::BlockedBy),
    ]
}

proptest! {
    #[test]
    fn display_round_trips_the_ast(
        terms in vec((any::<bool>(), clause()).prop_map(|(negated, clause)| Term { negated, clause }), 0..9)
    ) {
        let ast = Filter(terms);
        let text = ast.to_string();
        prop_assert_eq!(parse(&text, Date::MIN), Ok(ast), "canonical filter {:?}", text);
    }
}

#[test]
fn canonical_comparison_keeps_leading_equals_in_the_operand() {
    let filter = parse(r#"effort:<"=high",>"=low""#, Date::MIN).unwrap();
    let printed = filter.to_string();
    assert_eq!(printed, r#"effort:<"=high",>"=low""#);
    assert_eq!(parse(&printed, Date::MAX), Ok(filter));
}
