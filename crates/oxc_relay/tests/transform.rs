use std::path::Path;

use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_parser::Parser;
use oxc_relay::{Relay, RelayLanguage, RelayOptions};
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

fn transform(source_path: &str, source_text: &str, options: RelayOptions) -> (String, bool) {
    let source_type = SourceType::from_path(Path::new(source_path)).unwrap();
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source_text, source_type).parse();
    assert!(ret.diagnostics.is_empty(), "parse errors for source {source_text}");
    let mut program = ret.program;
    let scoping = SemanticBuilder::new().build(&program).semantic.into_scoping();
    let ret = Relay::new(options, Path::new(source_path)).build(&allocator, &mut program, scoping);
    let code = Codegen::new()
        .with_options(CodegenOptions { single_quote: true, ..CodegenOptions::default() })
        .build(&program)
        .code;
    (code, ret.diagnostics.has_errors())
}

fn codegen(source_path: &str, source_text: &str) -> String {
    let source_type = SourceType::from_path(Path::new(source_path)).unwrap();
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source_text, source_type).parse();
    assert!(ret.diagnostics.is_empty(), "parse errors for expected {source_text}");
    Codegen::new()
        .with_options(CodegenOptions { single_quote: true, ..CodegenOptions::default() })
        .build(&ret.program)
        .code
}

#[track_caller]
fn test_with_path(source_path: &str, source_text: &str, options: RelayOptions, expected: &str) {
    let (code, has_errors) = transform(source_path, source_text, options);
    assert!(!has_errors, "unexpected diagnostics for source {source_text}");
    assert_eq!(code, codegen(source_path, expected), "for source {source_text}");
}

#[track_caller]
fn test(source_text: &str, options: RelayOptions, expected: &str) {
    test_with_path("test.js", source_text, options, expected);
}

#[track_caller]
fn test_diagnostic(source_text: &str, options: RelayOptions) {
    let (code, has_errors) = transform("test.js", source_text, options);
    assert!(has_errors, "expected diagnostics for source {source_text}");
    // The node is left untouched.
    assert_eq!(code, codegen("test.js", source_text), "for source {source_text}");
}

fn require_options() -> RelayOptions {
    RelayOptions { eager_es_modules: false, ..RelayOptions::default() }
}

#[test]
fn eager_es_modules_default() {
    test(
        "const data = graphql`query FooQuery { id }`;",
        RelayOptions::default(),
        "import _FooQuery from './__generated__/FooQuery.graphql.js';
        const data = _FooQuery;",
    );
}

#[test]
fn eager_es_modules_multiple_tags() {
    test(
        "const a = graphql`query FooQuery { id }`;
        const b = graphql`fragment Bar_item on Item { id }`;",
        RelayOptions::default(),
        "import _FooQuery from './__generated__/FooQuery.graphql.js';
        import _Bar_item from './__generated__/Bar_item.graphql.js';
        const a = _FooQuery;
        const b = _Bar_item;",
    );
}

#[test]
fn eager_es_modules_uid_collisions() {
    // A user binding already named `_FooQuery`, and the same document twice.
    test(
        "const _FooQuery = 1;
        const a = graphql`query FooQuery { id }`;
        const b = graphql`query FooQuery { id }`;",
        RelayOptions::default(),
        "import _FooQuery2 from './__generated__/FooQuery.graphql.js';
        import _FooQuery3 from './__generated__/FooQuery.graphql.js';
        const _FooQuery = 1;
        const a = _FooQuery2;
        const b = _FooQuery3;",
    );
}

#[test]
fn require_call() {
    test(
        "const data = graphql`query FooQuery { id }`;",
        require_options(),
        "const data = require('./__generated__/FooQuery.graphql.js');",
    );
}

#[test]
fn definition_keywords() {
    for (document, name) in [
        ("query FooQuery { id }", "FooQuery"),
        ("mutation FooMutation { set }", "FooMutation"),
        ("subscription FooSubscription { event }", "FooSubscription"),
        ("fragment Foo_item on Item { id }", "Foo_item"),
    ] {
        test(
            &format!("const data = graphql`{document}`;"),
            require_options(),
            &format!("const data = require('./__generated__/{name}.graphql.js');"),
        );
    }
}

#[test]
fn artifact_directory() {
    // Sibling of the source directory.
    test_with_path(
        "project/src/pages/foo.js",
        "const data = graphql`query FooQuery { id }`;",
        RelayOptions {
            artifact_directory: Some("project/src/pages/__generated__".into()),
            ..require_options()
        },
        "const data = require('./__generated__/FooQuery.graphql.js');",
    );
    // Same directory as the source file.
    test_with_path(
        "project/src/foo.js",
        "const data = graphql`query FooQuery { id }`;",
        RelayOptions { artifact_directory: Some("project/src".into()), ..require_options() },
        "const data = require('./FooQuery.graphql.js');",
    );
    // Parent traversal, absolute paths.
    test_with_path(
        "/project/src/pages/foo.js",
        "const data = graphql`query FooQuery { id }`;",
        RelayOptions {
            artifact_directory: Some("/project/__generated__".into()),
            ..require_options()
        },
        "const data = require('../../__generated__/FooQuery.graphql.js');",
    );
}

#[test]
fn typescript_language() {
    test(
        "const data = graphql`query FooQuery { id }`;",
        RelayOptions { language: RelayLanguage::Typescript, ..require_options() },
        "const data = require('./__generated__/FooQuery.graphql.ts');",
    );
    test(
        "const data = graphql`query FooQuery { id }`;",
        RelayOptions { language: RelayLanguage::Flow, ..require_options() },
        "const data = require('./__generated__/FooQuery.graphql.js');",
    );
}

#[test]
fn name_in_later_quasi() {
    test(
        "const data = graphql`${directives} query FooQuery { id }`;",
        require_options(),
        "const data = require('./__generated__/FooQuery.graphql.js');",
    );
}

#[test]
fn comments() {
    // A commented-out definition does not count; the real one is found.
    test(
        "const data = graphql`# query Hidden\nmutation RealMutation { set }`;",
        require_options(),
        "const data = require('./__generated__/RealMutation.graphql.js');",
    );
    // Only a commented-out definition: diagnostic, node untouched.
    test_diagnostic("const data = graphql`# query Hidden`;", require_options());
}

#[test]
fn anonymous_definition_diagnostic() {
    test_diagnostic("const data = graphql`{ id }`;", require_options());
    test_diagnostic("const data = graphql`query { id }`;", RelayOptions::default());
    test_diagnostic("const data = graphql``;", require_options());
}

#[test]
fn non_graphql_tags_untouched() {
    let source = "const a = css`query FooQuery { id }`;
    const b = graphql.experimental`query FooQuery { id }`;
    const c = notgraphql`query FooQuery { id }`;";
    test(source, RelayOptions::default(), source);
}

#[test]
fn typescript_syntax_preserved() {
    test_with_path(
        "foo.ts",
        "interface Props { id: string }
        const data: Props = graphql`query FooQuery { id }`;",
        require_options(),
        "interface Props { id: string }
        const data: Props = require('./__generated__/FooQuery.graphql.js');",
    );
}

#[test]
fn jsx_syntax_preserved() {
    test_with_path(
        "foo.jsx",
        "const data = graphql`query FooQuery { id }`;
        export const App = () => <div>{data}</div>;",
        RelayOptions::default(),
        "import _FooQuery from './__generated__/FooQuery.graphql.js';
        const data = _FooQuery;
        export const App = () => <div>{data}</div>;",
    );
}
