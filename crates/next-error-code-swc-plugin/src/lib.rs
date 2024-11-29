use swc_core::{
    ecma::{ast::*, transforms::testing::test_inline, visit::*},
    plugin::{plugin_transform, proxies::TransformPluginProgramMetadata},
};

fn record_error_and_return_error_code(message: &str) -> String {
    use std::{
        collections::hash_map::DefaultHasher,
        fs,
        hash::{Hash, Hasher},
        path,
    };

    let mut hasher = DefaultHasher::new();
    message.hash(&mut hasher);
    let code = format!("E{}", hasher.finish());

    // Write error message to file if it doesn't exist
    let error_file_path = format!("/cwd/errors/{}.txt", code);
    if !path::Path::new("/cwd/errors").exists() {
        fs::create_dir_all("/cwd/errors")
            .unwrap_or_else(|e| panic!("Failed to create errors directory: {}", e));
    }

    if !path::Path::new(&error_file_path).exists() {
        let mut retries = 0;
        while retries < 3 {
            match fs::write(&error_file_path, message) {
                Ok(_) => break,
                Err(e) => {
                    if retries == 2 {
                        eprintln!(
                            "Failed to write error message to {} after 3 attempts: {}",
                            error_file_path, e
                        );
                    }
                    retries += 1;
                }
            }
        }
    }
    code
}

pub struct TransformVisitor;

impl VisitMut for TransformVisitor {
    fn visit_mut_new_expr(&mut self, expr: &mut NewExpr) {
        if let Expr::Ident(ident) = &*expr.callee {
            if ident.sym.to_string() == "Error" {
                fn traverse_expr(expr: &Expr) -> String {
                    match expr {
                        Expr::Lit(lit) => match lit {
                            Lit::Str(str_lit) => str_lit.value.to_string(),
                            _ => "%s".to_string(),
                        },

                        Expr::Tpl(tpl) => {
                            let mut result = String::new();
                            let mut expr_iter = tpl.exprs.iter();

                            for (_i, quasi) in tpl.quasis.iter().enumerate() {
                                result.push_str(&quasi.raw);
                                if let Some(expr) = expr_iter.next() {
                                    result.push_str(&traverse_expr(expr));
                                }
                            }
                            result
                        }

                        Expr::Bin(bin_expr) => {
                            // Assume binary expression is always add for two strings
                            format!(
                                "{}{}",
                                traverse_expr(&bin_expr.left),
                                traverse_expr(&bin_expr.right)
                            )
                        }

                        _ => "%s".to_string(),
                    }
                }

                if let Some(args) = &expr.args {
                    if let Some(first_arg) = args.first() {
                        let message: String = traverse_expr(&first_arg.expr);

                        // todo get the code and add error.code = ...
                        record_error_and_return_error_code(&message);
                    }
                }
            }
        }
    }
}

#[plugin_transform]
pub fn process_transform(program: Program, _metadata: TransformPluginProgramMetadata) -> Program {
    let mut visitor = TransformVisitor;
    visitor.visit_mut_program(&mut program.clone());
    program
}

// An example to test plugin transform.
// Recommended strategy to test plugin's transform is verify
// the Visitor's behavior, instead of trying to run `process_transform` with mocks
// unless explicitly required to do so.
test_inline!(
    Default::default(),
    |_| visit_mut_pass(TransformVisitor),
    boo,
    // Input codes
    r#"console.log(1 == 1);"#,
    // Output codes after transformed with plugin
    r#"console.log(1 === 1);"#
);
