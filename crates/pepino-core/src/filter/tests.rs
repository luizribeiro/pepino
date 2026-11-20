use super::*;

fn today() -> Date {
    "2026-11-19".parse().unwrap()
}

fn parse(input: &str) -> Result<Filter, ParseError> {
    super::parse(input, today())
}

fn parse_argv<S: AsRef<str>>(argv: &[S]) -> Result<Filter, ParseError> {
    super::parse_argv(argv, today())
}

#[test]
fn mixed_filter_has_one_canonical_spelling() {
    let input = r#" tag:garden  status:todo assignee:me due:<fri -tag:someday "fix the fence" -@"Weekend backlog" -has:parent is:blocked,ready effort:">threshold",>=5 "#;
    let argv = [
        "tag:garden",
        "status:todo",
        "assignee:me",
        "due:<fri",
        "-tag:someday",
        "fix the fence",
        "-@Weekend backlog",
        "-has:parent",
        "is:blocked,ready",
        r#"effort:">threshold",>=5"#,
    ];
    let expected = r#"tag:garden status:todo assignee:me due:<2026-11-20 -tag:someday "fix\u{20}the\u{20}fence" -@"Weekend\u{20}backlog" -has:parent is:blocked,ready effort:">threshold",>=5"#;
    let filter = parse(input).unwrap();
    assert_eq!(parse_argv(&argv).unwrap(), filter);
    assert_eq!(filter.to_string(), expected);
    assert_eq!(super::parse(expected, Date::MAX).unwrap(), filter);
    assert_eq!(Filter::default().to_string(), "");
}

#[test]
fn dates_resolve_across_weeks_and_years() {
    let today = "2027-12-31".parse().unwrap();
    let relative =
        "due:today,tomorrow,yesterday,fri,friday,mon,monday,+3d,-1w updated:<sunday created:>=+1w";
    let absolute = "due:2027-12-31,2028-01-01,2027-12-30,2027-12-31,2027-12-31,2028-01-03,2028-01-03,2028-01-03,2027-12-24 updated:<2028-01-02 created:>=2028-01-07";
    assert_eq!(
        super::parse(relative, today).unwrap(),
        super::parse(absolute, today).unwrap()
    );
    for value in ["+3", "-1m", "next week", "+ 3d", "2026-1-1"] {
        let error = super::parse_argv(&[format!("due:>{value}")], today).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::InvalidDate {
                value: value.into()
            }
        );
    }
    for (input, today) in [
        ("due:tomorrow", Date::MAX),
        ("due:-1d", Date::MIN),
        ("due:+9223372036854775807w", today),
        ("due:+99999999999999999999999d", today),
        ("due:+99999999999999999999999-01-01", today),
    ] {
        assert_eq!(
            super::parse(input, today).unwrap_err().kind,
            ErrorKind::DateOutOfRange {
                value: input[4..].into()
            }
        );
    }
    for (value, lowercase) in [("Fri", "fri"), ("+1W", "+1w")] {
        assert_eq!(
            parse(&format!("due:{value}")).unwrap_err().kind,
            ErrorKind::LowercaseKeyword {
                value: value.into(),
                lowercase: lowercase.into()
            }
        );
    }
}

#[test]
fn free_text_terms() {
    let filter = parse(r#"  garden "fix the \"split\" fence" -shed "#).unwrap();
    assert_eq!(
        filter
            .terms()
            .iter()
            .map(|t| (t.negated, &t.clause))
            .collect::<Vec<_>>(),
        [
            (false, &Clause::Text("garden".into())),
            (false, &Clause::Text("fix the \"split\" fence".into())),
            (true, &Clause::Text("shed".into()))
        ]
    );
    assert!(parse("  \n").unwrap().terms().is_empty());
}

#[test]
fn keyed_terms_preserve_quoted_delimiters() {
    let filter =
        parse(r#"tag:garden,"needs review","tools, seeds" "plot: north" -assignee:me"#).unwrap();
    assert_eq!(
        filter.terms()[0].clause,
        Clause::Match {
            key: "tag".into(),
            alternatives: ["garden", "needs review", "tools, seeds"]
                .map(|value| Comparison {
                    operator: Operator::Eq,
                    value: Value::Text(value.into()),
                })
                .into(),
        }
    );
    assert_eq!(filter.terms()[1].clause, Clause::Text("plot: north".into()));
    assert!(filter.terms()[2].negated);
    assert_eq!(
        parse_argv(&["tag:needs review"]),
        parse(r#"tag:"needs review""#)
    );
}

#[test]
fn calendar_dates_are_typed_and_validated() {
    let filter = parse("due:2026-12-01 created:>=2024-02-29 updated:<2027-01-01").unwrap();
    let expected = [
        ("due", Operator::Eq, "2026-12-01"),
        ("created", Operator::Ge, "2024-02-29"),
        ("updated", Operator::Lt, "2027-01-01"),
    ];
    for (term, (key, operator, date)) in filter.terms().iter().zip(expected) {
        assert_eq!(
            term.clause,
            Clause::Match {
                key: key.into(),
                alternatives: vec![Comparison {
                    operator,
                    value: Value::Date(date.parse().unwrap())
                }],
            }
        );
    }
    let error = parse("tag:garden due:2026-02-29").unwrap_err();
    assert_eq!(error.term, "due:2026-02-29");
    assert_eq!(
        error.kind,
        ErrorKind::InvalidDate {
            value: "2026-02-29".into()
        }
    );
}

#[test]
fn saved_view_references_stay_symbolic() {
    let filter = parse(r#"@weekend -@"garden backlog" "@supplier""#).unwrap();
    assert_eq!(filter.terms()[0].clause, Clause::View("weekend".into()));
    assert_eq!(
        filter.terms()[1],
        Term {
            negated: true,
            clause: Clause::View("garden backlog".into())
        },
    );
    assert_eq!(filter.terms()[2].clause, Clause::Text("@supplier".into()));
    assert_eq!(parse(r#"-@"""#).unwrap_err().kind, ErrorKind::EmptyView);
}

#[test]
fn predicates_accept_negation_and_alternatives() {
    let filter =
        parse(r#"is:blocked,ready -is:open,closed has:due,parent -has:assignee,"plant variety""#)
            .unwrap();
    assert_eq!(
        filter.terms()[0].clause,
        Clause::Is(vec![State::Blocked, State::Ready])
    );
    assert_eq!(
        filter.terms()[1].clause,
        Clause::Is(vec![State::Open, State::Closed])
    );
    assert_eq!(
        filter.terms()[2].clause,
        Clause::Has(vec!["due".into(), "parent".into()])
    );
    assert_eq!(
        filter.terms()[3].clause,
        Clause::Has(vec!["assignee".into(), "plant variety".into()])
    );
    assert!(filter.terms()[1].negated && filter.terms()[3].negated);
    assert_eq!(
        parse("is:stalled").unwrap_err().kind,
        ErrorKind::UnknownState {
            value: "stalled".into()
        }
    );
    assert_eq!(
        parse("has:priority").unwrap_err().kind,
        ErrorKind::UnsupportedPresence {
            field: "priority".into()
        }
    );
    assert_eq!(
        parse("is:>open").unwrap_err().kind,
        ErrorKind::UnsupportedComparison { key: "is".into() }
    );
}

#[test]
fn built_in_field_matches_preserve_symbolic_values() {
    let filter = parse("board:garden parent:ab12 blocks:cd34 blocked-by:ef56 assignee:me status:todo,doing,done,cancelled").unwrap();
    for (term, (name, value)) in filter.terms().iter().zip([
        ("board", "garden"),
        ("parent", "ab12"),
        ("blocks", "cd34"),
        ("blocked-by", "ef56"),
        ("assignee", "me"),
        ("status", "todo"),
    ]) {
        let Clause::Match { key, alternatives } = &term.clause else {
            panic!("expected match");
        };
        assert_eq!(key, name);
        assert_eq!(alternatives[0].value, Value::Text(value.into()));
    }

    let error = parse("tag:garden status:todo,in-progress").unwrap_err();
    assert_eq!(error.term, "status:todo,in-progress");
    assert_eq!(
        error.kind,
        ErrorKind::UnknownStatus {
            value: "in-progress".into()
        }
    );
    assert!(
        error
            .to_string()
            .contains("todo, doing, done, or cancelled")
    );
}

#[test]
fn comparison_operators_apply_to_each_alternative() {
    use Operator::*;
    let filter = parse(r#"priority:>=2,<4 effort:<=8,>12 tag:">gate""#).unwrap();
    let expected = [
        [(Ge, "2"), (Lt, "4")].as_slice(),
        &[(Le, "8"), (Gt, "12")],
        &[(Eq, ">gate")],
    ];
    for (term, pairs) in filter.terms().iter().zip(expected) {
        let Clause::Match { alternatives, .. } = &term.clause else {
            panic!("expected match");
        };
        assert_eq!(
            alternatives,
            &pairs
                .iter()
                .map(|&(operator, value)| Comparison {
                    operator,
                    value: Value::Text(value.into()),
                })
                .collect::<Vec<_>>()
        );
    }
    let error = parse("garden -tag:>=seeds").unwrap_err();
    assert_eq!(error.term, "-tag:>=seeds");
    assert_eq!(error.position, Position::Byte(12));
    assert_eq!(
        error.kind,
        ErrorKind::UnsupportedComparison { key: "tag".into() }
    );
    assert_eq!(
        parse("priority:>=").unwrap_err().kind,
        ErrorKind::EmptyValue
    );
}

#[test]
fn empty_keys_and_values_report_the_whole_term() {
    assert_eq!(parse(":garden").unwrap_err().kind, ErrorKind::EmptyKey);
    for raw in ["tag:", "tag:,garden", "tag:garden,", r#"tag:"""#] {
        let error = parse_argv(&[raw]).unwrap_err();
        assert_eq!(error.term, raw);
        assert_eq!(error.kind, ErrorKind::EmptyValue);
    }
}

#[test]
fn argv_keeps_each_argument_together() {
    let argv = ["garden", "fix the fence", "-paint the shed"];
    assert_eq!(
        parse_argv(&argv),
        parse(r#"garden "fix the fence" -"paint the shed""#)
    );
    assert!(parse_argv::<&str>(&[]).unwrap().terms().is_empty());

    let error = parse_argv(&["garden", r#""fix the fence"#]).unwrap_err();
    assert_eq!(error.position, Position::Argument { index: 1, byte: 0 });
    assert_eq!(error.term, r#""fix the fence"#);
    assert_eq!(error.kind, ErrorKind::UnterminatedQuote);
}

#[test]
fn invalid_text_terms_include_the_source() {
    for (input, offset, kind) in [
        (r#"café "fix fence"#, 6, ErrorKind::UnterminatedQuote),
        ("café -", 7, ErrorKind::EmptyTerm),
    ] {
        let error = parse(input).unwrap_err();
        assert_eq!(error.position, Position::Byte(offset));
        assert_eq!(error.term, input[6..]);
        assert_eq!(error.kind, kind);
    }
}

#[test]
fn weekdays_include_today_from_each_weekday() {
    let monday: Date = "2028-02-28".parse().unwrap();
    let weekdays = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let dates = [
        monday,
        monday.succ_opt().unwrap(),
        "2028-03-01".parse().unwrap(),
        "2028-03-02".parse().unwrap(),
        "2028-03-03".parse().unwrap(),
        "2028-03-04".parse().unwrap(),
        "2028-03-05".parse().unwrap(),
    ];
    for (start, today) in dates.into_iter().enumerate() {
        for (target, weekday) in weekdays.into_iter().enumerate() {
            let mut expected = dates[target];
            if target < start {
                expected += chrono::TimeDelta::days(7);
            }
            assert_eq!(
                super::parse(&format!("due:{weekday}"), today)
                    .unwrap()
                    .to_string(),
                format!("due:{expected}")
            );
        }
    }
}

#[test]
fn offsets_cross_calendar_boundaries() {
    for (today, relative, absolute) in [
        ("2028-02-28", "+1d", "2028-02-29"),
        ("2028-03-01", "-1d", "2028-02-29"),
        ("2027-02-28", "+1d", "2027-03-01"),
        ("2028-04-30", "+1d", "2028-05-01"),
        ("2028-01-02", "-1w", "2027-12-26"),
    ] {
        let today = today.parse().unwrap();
        assert_eq!(
            super::parse(&format!("due:{relative}"), today)
                .unwrap()
                .to_string(),
            format!("due:{absolute}")
        );
    }
}

#[test]
fn errors_point_inside_the_offending_term() {
    let error = parse("café due:today,<=broken status:todo").unwrap_err();
    assert_eq!(error.position, Position::Byte("café due:today,<=".len()));
    assert_eq!(error.term, "due:today,<=broken");
    let error = parse_argv(&["café", "due:today,<=broken"]).unwrap_err();
    assert_eq!(
        error.position,
        Position::Argument {
            index: 1,
            byte: "due:today,<=".len()
        }
    );
    let error = parse(r#"café date:today,"okay""open"#).unwrap_err();
    assert_eq!(
        error.position,
        Position::Byte(r#"café date:today,"okay""#.len())
    );
}

#[test]
fn whitespace_escapes_keep_canonical_terms_in_one_argument() {
    let text = r#""say \"hi\"\\there\n" -"<first" "a　b""#;
    let filter = parse(text).unwrap();
    assert_eq!(
        filter.terms()[0].clause,
        Clause::Text(r#"say "hi"\there\n"#.into())
    );
    let argv = filter
        .terms()
        .iter()
        .map(Term::to_string)
        .collect::<Vec<_>>();
    assert!(argv.iter().all(|term| !term.contains(char::is_whitespace)));
    assert_eq!(parse_argv(&argv), Ok(filter));
    assert_eq!(
        parse(r#""\\u{20} \u{xyz} \u{61}""#).unwrap().terms()[0].clause,
        Clause::Text(r"\u{20} \u{xyz} \u{61}".into())
    );
}
