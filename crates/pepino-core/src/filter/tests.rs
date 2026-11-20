use super::*;

#[test]
fn mixed_filter_has_one_canonical_spelling() {
    let input = r#" tag:garden  status:todo assignee:me due:<fri -tag:someday "fix the fence" -@weekend-backlog -has:parent is:blocked,ready effort:">threshold",>=5 "#;
    let argv = [
        "tag:garden",
        "status:todo",
        "assignee:me",
        "due:<fri",
        "-tag:someday",
        "fix the fence",
        "-@weekend-backlog",
        "-has:parent",
        "is:blocked,ready",
        r#"effort:">threshold",>=5"#,
    ];
    let expected = r#"tag:garden status:todo assignee:me due:<fri -tag:someday "fix the fence" -@weekend-backlog -has:parent is:blocked,ready effort:">threshold",>=5"#;
    let filter = parse(input).unwrap();
    assert_eq!(parse_argv(&argv).unwrap(), filter);
    assert_eq!(filter.to_string(), expected);
    assert_eq!(parse(expected).unwrap(), filter);
    assert_eq!(Filter::default().to_string(), "");
}

#[test]
fn dates_resolve_across_weeks_and_years() {
    let today = "2027-12-31".parse().unwrap();
    for (relative, absolute) in [
        ("today", "2027-12-31"),
        ("tomorrow", "2028-01-01"),
        ("yesterday", "2027-12-30"),
        ("fri", "2027-12-31"),
        ("friday", "2027-12-31"),
        ("mon", "2028-01-03"),
        ("monday", "2028-01-03"),
        ("+3d", "2028-01-03"),
        ("-1w", "2027-12-24"),
        ("sunday", "2028-01-02"),
        ("+1w", "2028-01-07"),
    ] {
        assert_eq!(
            relative.parse::<DateOperand>().unwrap().resolve(today),
            Ok(absolute.parse().unwrap()),
        );
    }
    for value in ["+3", "-1m", "next week", "+ 3d", "2026-1-1"] {
        let error = parse_argv(&[format!("due:>{value}")]).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::InvalidDate {
                value: value.into()
            }
        );
    }
    for (input, today) in [
        ("tomorrow", Date::MAX),
        ("-1d", Date::MIN),
        ("+999999999w", today),
        ("+9223372036854775807w", today),
        ("+99999999999999999999999d", today),
    ] {
        assert_eq!(
            input
                .parse::<DateOperand>()
                .unwrap()
                .resolve(today)
                .unwrap_err(),
            ErrorKind::DateOutOfRange {
                value: input.into()
            }
        );
    }
    let value = "+99999999999999999999999-01-01";
    assert_eq!(
        parse(&format!("due:{value}")).unwrap_err().kind,
        ErrorKind::DateOutOfRange {
            value: value.into()
        }
    );
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
        parse(r#"board:garden,"needs review","tools, seeds" "plot: north" -assignee:me"#).unwrap();
    assert_eq!(
        filter.terms()[0].clause,
        Clause::Board(
            ["garden", "needs review", "tools, seeds"]
                .map(str::to_owned)
                .into()
        )
    );
    assert_eq!(filter.terms()[1].clause, Clause::Text("plot: north".into()));
    assert!(filter.terms()[2].negated);
}

#[test]
fn tags_use_event_name_rules() {
    assert_eq!(
        parse("tag:garden.tools,home/repair,back-yard")
            .unwrap()
            .terms()[0]
            .clause,
        Clause::Tag(
            ["garden.tools", "home/repair", "back-yard"]
                .map(str::to_owned)
                .into()
        )
    );
    for value in ["needs review", "Upper", "9patch", "garden!"] {
        let error = parse_argv(&[format!("tag:{value}")]).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::InvalidTag {
                value: value.into()
            }
        );
        assert!(error.to_string().contains("[a-z][a-z0-9_./-]*"));
    }
}

#[test]
fn calendar_dates_are_typed_and_validated() {
    use Operator::*;
    let filter = parse("-due:<=2026-12-01,>fri created:>=2024-02-29 updated:<2027-01-01").unwrap();
    let comparison = |operator, date: &str| Comparison {
        operator,
        value: date.parse().unwrap(),
    };
    assert_eq!(
        filter.terms(),
        [
            Term {
                negated: true,
                clause: Clause::Due(vec![comparison(Le, "2026-12-01"), comparison(Gt, "fri"),])
            },
            Term {
                negated: false,
                clause: Clause::Created(vec![comparison(Ge, "2024-02-29")])
            },
            Term {
                negated: false,
                clause: Clause::Updated(vec![comparison(Lt, "2027-01-01")])
            },
        ]
    );
    let error = parse("tag:garden due:2026-02-29").unwrap_err();
    assert_eq!(&*error.term, "due:2026-02-29");
    assert_eq!(
        error.kind,
        ErrorKind::InvalidDate {
            value: "2026-02-29".into()
        }
    );
}

#[test]
fn saved_view_references_stay_symbolic() {
    let filter = parse(r#"@weekend -@garden-backlog "@supplier""#).unwrap();
    assert_eq!(filter.terms()[0].clause, Clause::View("weekend".into()));
    assert_eq!(
        filter.terms()[1],
        Term {
            negated: true,
            clause: Clause::View("garden-backlog".into())
        },
    );
    assert_eq!(filter.terms()[2].clause, Clause::Text("@supplier".into()));
    assert_eq!(parse(r#"-@"""#).unwrap_err().kind, ErrorKind::EmptyView);
}

#[test]
fn view_names_are_validated() {
    let invalid = r#"@"Weekend backlog""#;
    for error in [
        parse(invalid).unwrap_err(),
        parse_argv(&[invalid]).unwrap_err(),
    ] {
        assert_eq!(
            error.kind,
            ErrorKind::InvalidView {
                name: "Weekend backlog".into()
            }
        );
        assert!(
            error
                .to_string()
                .contains("View names cannot contain spaces")
        );
    }
    assert_eq!(parse("@").unwrap_err().kind, ErrorKind::EmptyView);
    assert!(parse(&format!("@{}", "a".repeat(64))).is_ok());
    assert!(parse(&format!("@{}", "a".repeat(65))).is_err());
}

#[test]
fn predicates_accept_negation_and_alternatives() {
    use crate::FieldName;

    let filter = parse("is:blocked,ready -is:open,closed has:due,parent,tag,description -has:assignee,priority,plant_variety").unwrap();
    let expected = [
        (false, Clause::Is(vec![State::Blocked, State::Ready])),
        (true, Clause::Is(vec![State::Open, State::Closed])),
        (
            false,
            Clause::Has(vec![
                PresenceField::Due,
                PresenceField::Parent,
                PresenceField::Tag,
                PresenceField::Description,
            ]),
        ),
        (
            true,
            Clause::Has(vec![
                PresenceField::Assignee,
                PresenceField::Priority,
                PresenceField::Custom(FieldName::new("plant_variety").unwrap()),
            ]),
        ),
    ]
    .map(|(negated, clause)| Term { negated, clause });
    assert_eq!(filter.terms(), expected);
    assert_eq!(
        parse("is:stalled").unwrap_err().kind,
        ErrorKind::UnknownState {
            value: "stalled".into()
        }
    );
    for (input, value, lowercase) in [("is:OPEN", "OPEN", "open"), ("has:Due", "Due", "due")] {
        assert_eq!(
            parse(input).unwrap_err().kind,
            ErrorKind::LowercaseKeyword {
                value: value.into(),
                lowercase: lowercase.into(),
            }
        );
    }
    for field in ["status", "id", "title", "statuses", "plant variety"] {
        assert_eq!(
            parse_argv(&[format!("has:{field}")]).unwrap_err().kind,
            ErrorKind::UnsupportedPresence {
                field: field.into()
            }
        );
    }
    assert_eq!(
        parse("is:>open").unwrap_err().kind,
        ErrorKind::UnsupportedComparison { key: "is".into() }
    );
}

#[test]
fn built_in_field_matches_preserve_symbolic_values() {
    use crate::Status::*;
    let filter = parse(r#"board:garden parent:ab12 blocks:cd34 blocked-by:ef56 assignee:me,"me",Ana status:todo,doing,done,cancelled"#).unwrap();
    let expected = [
        Clause::Board(vec!["garden".into()]),
        Clause::Parent(vec!["ab12".into()]),
        Clause::Blocks(vec!["cd34".into()]),
        Clause::BlockedBy(vec!["ef56".into()]),
        Clause::Assignee(vec![
            Assignee::Me,
            Assignee::Person("me".into()),
            Assignee::Person("Ana".into()),
        ]),
        Clause::Status(vec![Todo, Doing, Done, Cancelled]),
    ];
    assert_eq!(
        filter.terms().iter().map(|t| &t.clause).collect::<Vec<_>>(),
        expected.iter().collect::<Vec<_>>()
    );
    assert!(filter.to_string().contains(r#"assignee:me,"me",Ana"#));
    assert_eq!(
        parse("status:TODO").unwrap_err().kind,
        ErrorKind::LowercaseKeyword {
            value: "TODO".into(),
            lowercase: "todo".into()
        }
    );

    let error = parse("tag:garden status:todo,in-progress").unwrap_err();
    assert_eq!(&*error.term, "status:todo,in-progress");
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
    use crate::FieldName;
    use Operator::*;
    let filter = parse("priority:>=2,<4 effort:<=8,>12").unwrap();
    assert_eq!(
        filter.terms(),
        [
            Term {
                negated: false,
                clause: Clause::Priority(vec![
                    Comparison {
                        operator: Ge,
                        value: 2
                    },
                    Comparison {
                        operator: Lt,
                        value: 4
                    },
                ])
            },
            Term {
                negated: false,
                clause: Clause::Custom {
                    field: FieldName::new("effort").unwrap(),
                    alternatives: vec![
                        Comparison {
                            operator: Le,
                            value: CustomValue::Integer(8)
                        },
                        Comparison {
                            operator: Gt,
                            value: CustomValue::Integer(12)
                        },
                    ],
                }
            },
        ]
    );
    for value in ["high", "9223372036854775808"] {
        assert_eq!(
            parse(&format!("priority:{value}")).unwrap_err().kind,
            ErrorKind::InvalidInteger {
                value: value.into()
            }
        );
    }
    let error = parse("garden -tag:>=seeds").unwrap_err();
    assert_eq!(&*error.term, "-tag:>=seeds");
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
fn empty_values_report_the_whole_term() {
    for raw in ["tag:", "tag:,garden", "tag:garden,", r#"tag:"""#] {
        let error = parse_argv(&[raw]).unwrap_err();
        assert_eq!(&*error.term, raw);
        assert_eq!(error.kind, ErrorKind::EmptyValue);
    }
}

#[test]
fn argv_keeps_each_argument_together() {
    let argv = [
        "garden",
        " fix: the fence　",
        "-5 degrees",
        r#"say "hi""#,
        "@garden backlog",
        r#""fix the fence"#,
        "re: fence",
        "http://example.com",
    ];
    let expected = [
        "garden",
        "fix: the fence",
        "-5 degrees",
        r#"say "hi""#,
        "@garden backlog",
        r#""fix the fence"#,
        "re: fence",
        "http://example.com",
    ];
    let filter = parse_argv(&argv).unwrap();
    assert_eq!(
        filter.terms(),
        expected.map(|text| Term {
            negated: false,
            clause: Clause::Text(text.into())
        })
    );
    assert!(parse_argv::<&str>(&[]).unwrap().terms().is_empty());
    assert_eq!(
        parse_argv(&[" 　 "]).unwrap_err().kind,
        ErrorKind::EmptyTerm
    );

    let error = parse_argv(&["garden", r#" "broken"#]).unwrap_err();
    assert_eq!(error.position, Position::Argument { index: 1, byte: 1 });
    assert_eq!(&*error.term, r#""broken"#);
    assert_eq!(error.kind, ErrorKind::UnterminatedQuote);
}

#[test]
fn argv_parses_keyed_values_with_whitespace() {
    let argv = [
        " assignee:Ana Maria ",
        r#"-board:House Move,"Tools, Seeds""#,
        "client:Acme Corp",
        "@garden",
    ];
    let input =
        r#"assignee:"Ana Maria" -board:"House Move","Tools, Seeds" client:"Acme Corp" @garden"#;
    let filter = parse_argv(&argv).unwrap();
    assert_eq!(filter.to_string(), input);
    assert_eq!(filter, parse(input).unwrap());
    assert_eq!(
        parse("board:\"//Tools\"").unwrap().to_string(),
        "board:\"//Tools\""
    );
}

#[test]
fn unicode_whitespace_separates_string_terms() {
    assert_eq!(
        parse("　garden\u{2003}board:yard\u{a0}-shed\n"),
        parse("garden board:yard -shed")
    );
}

#[test]
fn invalid_text_terms_include_the_source() {
    for (input, offset, kind) in [
        (r#"café "fix fence"#, 6, ErrorKind::UnterminatedQuote),
        ("café -", 7, ErrorKind::EmptyTerm),
    ] {
        let error = parse(input).unwrap_err();
        assert_eq!(error.position, Position::Byte(offset));
        assert_eq!(&*error.term, &input[6..]);
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
                weekday.parse::<DateOperand>().unwrap().resolve(today),
                Ok(expected)
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
            relative.parse::<DateOperand>().unwrap().resolve(today),
            Ok(absolute.parse().unwrap())
        );
    }
}

#[test]
fn errors_point_inside_the_offending_term() {
    let error = parse("café due:today,<=broken status:todo").unwrap_err();
    assert_eq!(error.position, Position::Byte("café due:today,<=".len()));
    assert_eq!(&*error.term, "due:today,<=broken");
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
fn canonical_quotes_preserve_literal_whitespace() {
    let text = concat!(
        r#""say \"hi\"\\there\\n" -"<first" "a　b" "#,
        "\"line one\nline two\tend\""
    );
    let filter = parse(text).unwrap();
    assert_eq!(
        filter.terms()[0].clause,
        Clause::Text(r#"say "hi"\there\n"#.into())
    );
    assert_eq!(filter.to_string(), text);
    assert_eq!(
        parse(r#""\u{20} \n""#).unwrap().terms()[0].clause,
        Clause::Text(r"\u{20} \n".into())
    );
}

#[test]
fn keys_use_field_tokens_and_suggest_builtins() {
    for literal in [
        "http://example.com",
        ":garden",
        "bad.key:pear",
        r#""tag":garden"#,
        "id:42",
    ] {
        let expected = if literal == r#""tag":garden"# {
            "tag:garden"
        } else {
            literal
        };
        assert_eq!(
            parse(literal).unwrap().terms()[0].clause,
            Clause::Text(expected.into())
        );
    }
    assert_eq!(
        parse("Tag:x").unwrap_err().kind,
        ErrorKind::LowercaseKeyword {
            value: "Tag".into(),
            lowercase: "tag".into(),
        }
    );
    for (value, suggested) in [
        ("tags", "tag"),
        ("assignees", "assignee"),
        ("statuses", "status"),
    ] {
        assert_eq!(
            parse(&format!("{value}:x")).unwrap_err().kind,
            ErrorKind::UnknownKey {
                value: value.into(),
                suggested,
            }
        );
    }
    assert_eq!(parse("re: fence").unwrap_err().kind, ErrorKind::EmptyValue);
}

#[test]
fn custom_operands_use_their_unquoted_form() {
    use crate::FieldName;
    use Operator::*;
    let input = r#"reminder:today,>=+3d,"today","fri","+3d","-8",later,<=2028-02-29"#;
    let comparison = |operator, value| Comparison { operator, value };
    let filter = parse(input).unwrap();
    assert_eq!(
        filter.terms(),
        [Term {
            negated: false,
            clause: Clause::Custom {
                field: FieldName::new("reminder").unwrap(),
                alternatives: vec![
                    comparison(Eq, CustomValue::Date("today".parse().unwrap())),
                    comparison(Ge, CustomValue::Date("+3d".parse().unwrap())),
                    comparison(Eq, CustomValue::Text("today".into())),
                    comparison(Eq, CustomValue::Text("fri".into())),
                    comparison(Eq, CustomValue::Text("+3d".into())),
                    comparison(Eq, CustomValue::Text("-8".into())),
                    comparison(Eq, CustomValue::Text("later".into())),
                    comparison(Le, CustomValue::Date("2028-02-29".parse().unwrap())),
                ],
            },
        }]
    );
    assert_eq!(parse(&filter.to_string()), Ok(filter));
    for (input, kind) in [
        (
            "effort:+9223372036854775808",
            ErrorKind::InvalidInteger {
                value: "+9223372036854775808".into(),
            },
        ),
        (
            "reminder:+999999999-01-01",
            ErrorKind::DateOutOfRange {
                value: "+999999999-01-01".into(),
            },
        ),
    ] {
        assert_eq!(parse(input).unwrap_err().kind, kind);
    }
}

#[test]
fn filters_can_be_constructed_from_terms() {
    let terms = vec![
        Term {
            negated: false,
            clause: Clause::Status(vec![crate::Status::Todo]),
        },
        Term {
            negated: true,
            clause: Clause::Has(vec![PresenceField::Assignee]),
        },
    ];
    let filter = Filter::from_terms(terms.clone());
    assert_eq!(filter.terms(), terms);
    assert_eq!(parse("status:todo -has:assignee"), Ok(filter));
}

#[test]
fn each_error_kind_has_a_display_message() {
    let examples = [
        (
            r#""broken"#,
            "unterminated quote. Add a closing double quote",
        ),
        ("-", "empty term. Supply a search term"),
        (
            "@",
            "empty view reference. Supply a saved view name after '@'",
        ),
        (
            r#"@"Weekend backlog""#,
            "invalid saved view name \"Weekend backlog\". View names cannot contain spaces and must match [a-z][a-z0-9_-]* within 64 bytes",
        ),
        ("tags:tools", "unknown filter key \"tags\". Use \"tag\""),
        ("board:", "empty value. Supply a value for each alternative"),
        (
            "tag:<tools",
            "comparisons are unavailable for \"tag\". Use tag:value for equality",
        ),
        (
            "status:paused",
            "unknown status \"paused\". Use todo, doing, done, or cancelled",
        ),
        (
            r#"tag:"needs review""#,
            "invalid tag \"needs review\". Tags must match [a-z][a-z0-9_./-]*",
        ),
        (
            "is:stalled",
            "unknown state \"stalled\". Use blocked, ready, open, or closed",
        ),
        (
            "has:board",
            "unsupported presence field \"board\". Use due, parent, assignee, priority, tag, description, or a custom field",
        ),
        (
            r#"due:"+ 3d""#,
            "invalid date \"+ 3d\". Use YYYY-MM-DD, today, tomorrow, yesterday, a weekday, or an offset such as +3d",
        ),
        (
            "priority:high",
            "invalid integer \"high\". Enter a signed 64-bit integer",
        ),
        (
            "due:+999999999-01-01",
            "date \"+999999999-01-01\" is outside the supported calendar range",
        ),
        (
            "status:TODO",
            "keywords are lowercase. Use \"todo\" for \"TODO\"",
        ),
    ];
    for (input, expected) in examples {
        assert_eq!(
            parse(input).unwrap_err().kind.to_string(),
            expected,
            "{input:?}"
        );
    }
    let error = parse_argv(&["garden", " -due:<Fri "]).unwrap_err();
    assert_eq!(
        error.to_string(),
        r#"filter term "-due:<Fri" at argument 1, byte 7: keywords are lowercase. Use "fri" for "Fri""#
    );
}

#[test]
fn diagnostics_keep_punctuation_inside_the_reason() {
    let error = parse(r#"tag:hardware tag:"spare. parts""#).unwrap_err();
    assert_eq!(error.position, Position::Byte("tag:hardware tag:".len()));
    assert_eq!(&*error.reason, r#"invalid tag "spare. parts""#);
    assert_eq!(
        error.hint.as_deref(),
        Some("Tags must match [a-z][a-z0-9_./-]*")
    );
}
