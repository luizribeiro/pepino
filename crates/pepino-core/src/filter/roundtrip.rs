use proptest::{collection::vec, prelude::*, sample::select};

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
    let states = select(vec![
        State::Blocked,
        State::Ready,
        State::Open,
        State::Closed,
    ]);
    let presence = prop_oneof![
        named(vec!["due", "parent", "assignee"]),
        atom().prop_map(|name| format!("field {name}")),
    ];
    let custom_key = prop_oneof![
        Just("priority".into()),
        atom().prop_map(|name| format!("field {name}")),
    ];
    let dates = prop_oneof![
        select(vec![Date::MIN, Date::MAX]),
        (-200_000..=200_000, 1_u32..=12, 1_u32..=28)
            .prop_map(|(y, m, d)| Date::from_ymd_opt(y, m, d).unwrap()),
    ];
    prop_oneof![
        atom().prop_map(Clause::Text),
        atom().prop_map(Clause::View),
        vec(states, 1..4).prop_map(Clause::Is),
        vec(presence, 1..4).prop_map(Clause::Has),
        matching(custom_key, (operator(), atom().prop_map(Value::Text))),
        matching(
            named(vec!["due", "created", "updated"]),
            (operator(), dates.prop_map(Value::Date))
        ),
        matching(
            named(vec![
                "tag",
                "assignee",
                "board",
                "parent",
                "blocks",
                "blocked-by"
            ]),
            (Just(Operator::Eq), atom().prop_map(Value::Text)),
        ),
        matching(
            Just("status".into()),
            (
                Just(Operator::Eq),
                named(vec!["todo", "doing", "done", "cancelled"]).prop_map(Value::Text)
            ),
        ),
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
