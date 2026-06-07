//! Macro rewrites over a parsed [`Filter`]'s AST.
//!
//! A macro is any `Fn(&mut Cursor)`: [`apply_macros`] walks the tree and offers
//! each node to every macro, which may [`Cursor::replace`] it with a rewritten
//! subtree (e.g. desugaring `a.b = c` into a has-style call). After the walk the
//! result is re-type-checked against the supplied declarations, so a rewrite
//! that produces an ill-typed tree is rejected.

use crate::{checker, Declarations, Error, Expr, Filter};

/// A cursor positioned at one expression while [`apply_macros`] walks the AST.
///
/// A macro inspects [`Cursor::expr`] and, when it matches, calls
/// [`Cursor::replace`] to rewrite the node in place.
pub struct Cursor<'a> {
    expr: &'a mut Expr,
    replaced: bool,
}

impl<'a> Cursor<'a> {
    fn new(expr: &'a mut Expr) -> Self {
        Self {
            expr,
            replaced: false,
        }
    }

    /// The expression the cursor is positioned at.
    pub fn expr(&self) -> &Expr {
        self.expr
    }

    /// Replace the current expression with `new`. The walk does not descend into
    /// the replacement and no further macros run at this position.
    pub fn replace(&mut self, new: Expr) {
        *self.expr = new;
        self.replaced = true;
    }
}

/// Apply `macros` to `filter`'s AST, then re-type-check against `declarations`.
///
/// Every node is visited in depth-first pre-order. At each node the macros run
/// in order until one replaces it; after a replacement the node's (new) children
/// are not visited and the remaining macros are skipped for that node. The
/// rewritten filter is re-checked against `declarations`, which may differ from
/// the declarations the filter was originally checked with — a rewrite into an
/// undeclared identifier or an unresolvable overload is an [`Error`].
pub fn apply_macros(
    mut filter: Filter,
    declarations: &Declarations,
    macros: &[&dyn Fn(&mut Cursor)],
) -> Result<Filter, Error> {
    apply(&mut filter.expr, macros);
    checker::check(&filter.expr, declarations)?;
    Ok(filter)
}

/// Visit `expr` and its descendants, applying `macros` (see [`apply_macros`]).
fn apply(expr: &mut Expr, macros: &[&dyn Fn(&mut Cursor)]) {
    {
        let mut cursor = Cursor::new(expr);
        for &macro_fn in macros {
            macro_fn(&mut cursor);
            if cursor.replaced {
                // Don't descend into a replaced node.
                return;
            }
        }
    }
    match expr {
        Expr::Select { operand, .. } => apply(operand, macros),
        Expr::Call { args, .. } => {
            for arg in args.iter_mut() {
                apply(arg, macros);
            }
        }
        Expr::Const(_) | Expr::Ident(_) => {}
    }
}
