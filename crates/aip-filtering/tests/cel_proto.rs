//! Round-trip tests for the optional `cel-proto` conversion: the native AST
//! lowers to the `google.api.expr.v1alpha1` CEL protos and back, preserving
//! structure. Compiled only with `--features cel-proto`.
#![cfg(feature = "cel-proto")]

use std::collections::HashSet;

use aip_filtering::cel_proto::cel::{self, expr::ExprKind};
use aip_filtering::cel_proto::ConversionError;
use aip_filtering::{check, function, Constant, Declarations, Expr, Filter};

/// Every native constant kind survives `Constant -> cel::Constant -> Constant`
/// and, wrapped as an expression, `Expr -> cel::Expr -> Expr`.
#[test]
fn round_trips_every_constant_kind() {
    let constants = [
        Constant::Int(-7),
        Constant::Uint(42),
        Constant::Double(3.5),
        Constant::Bool(true),
        Constant::String("urgent".to_owned()),
        Constant::Bytes(vec![0x00, 0x01, 0xff]),
    ];

    for constant in constants {
        let proto: cel::Constant = (&constant).into();
        let back: Constant = proto.try_into().expect("constant round-trips");
        assert_eq!(constant, back);

        let expr = Expr::Const(constant.clone());
        let proto_expr: cel::Expr = (&expr).into();
        let back_expr: Expr = proto_expr.try_into().expect("const expr round-trips");
        assert_eq!(expr, back_expr);
    }
}

/// A tree covering every native node kind — `Call` (nested), `Select`, `Ident`,
/// and `Const` — round-trips through `cel::Expr` unchanged.
#[test]
fn round_trips_a_full_expression_tree() {
    let expr = Expr::Call {
        function: function::AND.to_owned(),
        args: vec![
            Expr::Call {
                function: function::EQUALS.to_owned(),
                args: vec![
                    Expr::Select {
                        operand: Box::new(Expr::Ident("resource".to_owned())),
                        field: "size".to_owned(),
                    },
                    Expr::Const(Constant::Int(10)),
                ],
            },
            Expr::Call {
                function: function::HAS.to_owned(),
                args: vec![
                    Expr::Ident("tags".to_owned()),
                    Expr::Const(Constant::String("urgent".to_owned())),
                ],
            },
        ],
    };

    let proto: cel::Expr = (&expr).into();
    let back: Expr = proto.try_into().expect("tree round-trips");
    assert_eq!(expr, back);
}

/// Native → proto stamps every node with a unique, non-zero id (CEL requires
/// non-zero ids; reused ids would corrupt a CheckedExpr's side tables).
#[test]
fn assigns_unique_nonzero_ids() {
    // 8 nodes: AND, =, Select, Ident(resource), Const(10), :, Ident(tags),
    // Const("urgent").
    let expr = Expr::Call {
        function: function::AND.to_owned(),
        args: vec![
            Expr::Call {
                function: function::EQUALS.to_owned(),
                args: vec![
                    Expr::Select {
                        operand: Box::new(Expr::Ident("resource".to_owned())),
                        field: "size".to_owned(),
                    },
                    Expr::Const(Constant::Int(10)),
                ],
            },
            Expr::Call {
                function: function::HAS.to_owned(),
                args: vec![
                    Expr::Ident("tags".to_owned()),
                    Expr::Const(Constant::String("urgent".to_owned())),
                ],
            },
        ],
    };

    let proto: cel::Expr = (&expr).into();
    let mut ids = HashSet::new();
    collect_ids(&proto, &mut ids);

    assert_eq!(ids.len(), 8, "every one of the 8 nodes has its own id");
    assert!(!ids.contains(&0), "no node uses the reserved id 0");
}

/// A realistic parsed-and-checked filter round-trips as a `CheckedExpr`,
/// preserving its expression tree (the `type_map` is intentionally empty).
#[test]
fn round_trips_a_checked_filter() {
    let declarations: Declarations = Declarations::builder()
        .standard_functions()
        .ident("package", aip_filtering::Type::String)
        .ident(
            "com",
            aip_filtering::Type::Map(
                Box::new(aip_filtering::Type::String),
                Box::new(aip_filtering::Type::String),
            ),
        )
        .ident("size", aip_filtering::Type::Int)
        .build();

    let filter =
        check("package = com.google AND size > 10", &declarations).expect("filter type-checks");

    let proto: cel::CheckedExpr = (&filter).into();
    assert!(proto.expr.is_some(), "the checked expr carries the tree");
    assert!(
        proto.type_map.is_empty() && proto.reference_map.is_empty(),
        "native filters carry no per-node CEL types",
    );

    let back: Filter = proto.try_into().expect("checked expr round-trips");
    assert_eq!(filter.expr, back.expr);
}

/// Proto → native rejects every CEL node the native AST cannot represent rather
/// than silently reshaping it.
#[test]
fn rejects_unsupported_proto_nodes() {
    // Constant kinds with no native equivalent.
    assert!(matches!(
        Constant::try_from(null_constant()),
        Err(ConversionError::UnsupportedConstant(_)),
    ));
    assert!(matches!(
        Constant::try_from(cel::Constant {
            constant_kind: None
        }),
        Err(ConversionError::Missing(_)),
    ));

    // Expression kinds with no native equivalent.
    assert!(matches!(
        Expr::try_from(wrap(ExprKind::ListExpr(cel::expr::CreateList::default()))),
        Err(ConversionError::UnsupportedExpr(_)),
    ));
    assert!(matches!(
        Expr::try_from(wrap(ExprKind::StructExpr(
            cel::expr::CreateStruct::default()
        ))),
        Err(ConversionError::UnsupportedExpr(_)),
    ));
    assert!(matches!(
        Expr::try_from(wrap(ExprKind::ComprehensionExpr(Box::default()))),
        Err(ConversionError::UnsupportedExpr(_)),
    ));

    // A method-style call (has a target) is not function-style.
    let method_call = wrap(ExprKind::CallExpr(Box::new(cel::expr::Call {
        target: Some(Box::new(ident("x"))),
        function: "f".to_owned(),
        args: vec![],
    })));
    assert!(matches!(
        Expr::try_from(method_call),
        Err(ConversionError::UnsupportedExpr(_)),
    ));

    // A presence-test select (`has(x.y)`) would change meaning as a plain field.
    let test_only = wrap(ExprKind::SelectExpr(Box::new(cel::expr::Select {
        operand: Some(Box::new(ident("x"))),
        field: "y".to_owned(),
        test_only: true,
    })));
    assert!(matches!(
        Expr::try_from(test_only),
        Err(ConversionError::UnsupportedExpr(_)),
    ));

    // An unset expr oneof, and a CheckedExpr with no expr.
    assert!(matches!(
        Expr::try_from(cel::Expr {
            id: 1,
            expr_kind: None,
        }),
        Err(ConversionError::Missing(_)),
    ));
    assert!(matches!(
        Filter::try_from(cel::CheckedExpr::default()),
        Err(ConversionError::Missing(_)),
    ));
}

/// Recursively gather the `id` of every node in a `cel::Expr`.
fn collect_ids(expr: &cel::Expr, ids: &mut HashSet<i64>) {
    ids.insert(expr.id);
    match &expr.expr_kind {
        Some(ExprKind::SelectExpr(select)) => {
            if let Some(operand) = &select.operand {
                collect_ids(operand, ids);
            }
        }
        Some(ExprKind::CallExpr(call)) => {
            if let Some(target) = &call.target {
                collect_ids(target, ids);
            }
            for arg in &call.args {
                collect_ids(arg, ids);
            }
        }
        _ => {}
    }
}

/// A `cel::Expr` carrying `kind` with an arbitrary id.
fn wrap(kind: ExprKind) -> cel::Expr {
    cel::Expr {
        id: 1,
        expr_kind: Some(kind),
    }
}

/// A `cel::Expr` identifier node.
fn ident(name: &str) -> cel::Expr {
    wrap(ExprKind::IdentExpr(cel::expr::Ident {
        name: name.to_owned(),
    }))
}

/// A CEL `null` constant.
fn null_constant() -> cel::Constant {
    cel::Constant {
        constant_kind: Some(cel::constant::ConstantKind::NullValue(0)),
    }
}
