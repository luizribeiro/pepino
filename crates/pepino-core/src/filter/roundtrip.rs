use chrono::Datelike;
use proptest::{collection::vec, prelude::*, sample::select};

use crate::FieldName;

use super::keywords::{Builtin, STATUSES};
use super::*;

fn atom() -> impl Strategy<Value = String> {
    let punctuation = select(vec![
        ' ', '\t', '\n', '\u{2003}', '"', '\\', ':', ',', '<', '>', '=', '-', '@',
    ]);
    prop_oneof![
        vec(prop_oneof![1 => any::<char>(), 3 => punctuation], 1..16)
            .prop_map(|chars| chars.into_iter().collect()),
        select(vec![
            " ",
            "\t",
            "\u{2003}",
            "today",
            "fri",
            "+3d",
            "-5",
            "@garden",
            "<gate",
            ">gate",
            "=gate",
            "tag:tools",
            "a,b",
            r#"say "hi""#,
            r"a\b",
            r"\n",
            "//parts",
            "2026-11-19",
            "9223372036854775808"
        ])
        .prop_map(str::to_owned),
    ]
}

fn operator() -> impl Strategy<Value = Operator> {
    use Operator::*;
    select(vec![Eq, Lt, Le, Gt, Ge])
}

fn field_names() -> impl Strategy<Value = FieldName> {
    "[a-z][a-z0-9_-]{0,63}".prop_filter_map("valid custom filter key", |name| {
        if Builtin::correction(&name).is_some() {
            return None;
        }
        FieldName::new(name).ok()
    })
}

fn comparisons<T: std::fmt::Debug>(
    value: impl Strategy<Value = T>,
) -> impl Strategy<Value = Vec<Comparison<T>>> {
    vec(
        (operator(), value).prop_map(|(operator, value)| Comparison { operator, value }),
        1..4,
    )
}

fn dates() -> impl Strategy<Value = Date> {
    prop_oneof![
        select(vec![Date::MIN, Date::MAX]),
        (Date::MIN.num_days_from_ce()..=Date::MAX.num_days_from_ce())
            .prop_map(|day| Date::from_num_days_from_ce_opt(day).unwrap()),
    ]
}

fn date_operands() -> impl Strategy<Value = DateOperand> {
    prop_oneof![
        dates().prop_map(DateOperand::Absolute),
        select(vec![
            "today",
            "tomorrow",
            "yesterday",
            "mon",
            "tue",
            "wed",
            "thu",
            "fri",
            "sat",
            "sun",
            "+99999999999999999999999d",
        ])
        .prop_map(|relative| relative.parse().unwrap()),
        (any::<i64>(), select(vec!['d', 'w'])).prop_map(|(amount, unit)| format!(
            "{amount:+}{unit}"
        )
        .parse()
        .unwrap()),
    ]
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
        field_names()
            .prop_filter("custom presence field", FieldName::is_custom)
            .prop_map(PresenceField::Custom),
    ];
    let custom_operand = prop_oneof![
        any::<i64>().prop_map(CustomValue::Integer),
        date_operands().prop_map(CustomValue::Date),
        atom().prop_map(CustomValue::Text),
    ];
    prop_oneof![
        atom().prop_map(Clause::Text),
        "[a-z][a-z0-9_-]{0,63}".prop_map(Clause::View),
        vec(states, 1..4).prop_map(Clause::Is),
        vec(presence, 1..4).prop_map(Clause::Has),
        (field_names(), comparisons(custom_operand)).prop_map(|(field, alternatives)| {
            Clause::Custom {
                field,
                alternatives,
            }
        }),
        comparisons(any::<i64>()).prop_map(Clause::Priority),
        comparisons(date_operands()).prop_map(Clause::Due),
        comparisons(date_operands()).prop_map(Clause::Created),
        comparisons(date_operands()).prop_map(Clause::Updated),
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
        let ast = Filter::from_terms(terms);
        let text = ast.to_string();
        let parsed = parse(&text);
        prop_assert_eq!(parsed.as_ref(), Ok(&ast), "canonical filter {:?}", text);

        let argv = ast.terms().iter().map(Term::to_string)
            .filter(|term| !term.contains(char::is_whitespace))
            .collect::<Vec<_>>();
        prop_assert_eq!(parse_argv(&argv), parse(&argv.join(" ")), "canonical argv {:?}", argv);
    }

    #[test]
    fn arbitrary_string_parses_without_panicking_with_fixed_point_display(
        input in any::<String>(),
    ) {
        if let Ok(ast) = parse(&input) {
            let printed = ast.to_string();
            let reparsed = parse(&printed);
            prop_assert_eq!(reparsed.as_ref(), Ok(&ast));
            prop_assert_eq!(reparsed.unwrap().to_string(), printed);
        }
    }

    #[test]
    fn arbitrary_argv_parses_without_panicking(
        argv in vec(any::<String>(), 0..16),
    ) {
        if let Ok(ast) = parse_argv(&argv) {
            prop_assert_eq!(parse(&ast.to_string()), Ok(ast));
        }
    }
}

#[test]
fn canonical_comparison_keeps_leading_equals_in_the_operand() {
    let filter = parse(r#"effort:<"=high",>"=low""#).unwrap();
    let printed = filter.to_string();
    assert_eq!(printed, r#"effort:<"=high",>"=low""#);
    assert_eq!(parse(&printed), Ok(filter));
}
