use super::*;

fn project(main: &str, files: &[(&str, &str)]) -> Project {
    let mut sources = files
        .iter()
        .map(|(name, source)| (name.to_string(), source.to_string()))
        .collect::<BTreeMap<_, _>>();
    sources.insert(DEFAULT_MAIN_FILE.to_string(), main.to_string());
    Project {
        main: DEFAULT_MAIN_FILE.to_string(),
        files: sources,
    }
}

fn evaluate_project(project: &Project) -> Result<Shape, EngineError> {
    let program = parse_resolved_program(project)?;
    Evaluator::default().evaluate_program(&program)
}

fn assert_inside(shape: &Shape, point: Vec3) {
    assert!(
        shape.distance(point) < 0.0,
        "expected {point:?} to be inside the evaluated shape"
    );
}

fn assert_outside(shape: &Shape, point: Vec3) {
    assert!(
        shape.distance(point) > 0.0,
        "expected {point:?} to be outside the evaluated shape"
    );
}

fn max_x(mesh: &Mesh) -> f64 {
    mesh.triangles
        .iter()
        .flat_map(|triangle| triangle.vertices)
        .map(|vertex| vertex.x)
        .fold(f64::NEG_INFINITY, f64::max)
}

#[test]
fn include_reads_a_workspace_file() {
    let shape = evaluate_project(&project(
        "include <params.scad>\ncube(size);",
        &[("params.scad", "size = 7;")],
    ))
    .expect("include");
    assert_inside(&shape, Vec3::new(6.5, 6.5, 6.5));
    assert_outside(&shape, Vec3::new(7.5, 1.0, 1.0));
}

#[test]
fn use_reads_a_workspace_file() {
    let shape = evaluate_project(&project(
        "use <parts.scad>\npeg();",
        &[("parts.scad", "module peg() cylinder(h=10, r=2);\npeg();")],
    ))
    .expect("use");
    assert_inside(&shape, Vec3::new(0.0, 0.0, 9.0));
    assert_outside(&shape, Vec3::new(0.0, 0.0, 11.0));
}

#[test]
fn workspace_files_shadow_bundled_libraries_even_from_inside_them() {
    // Bundled `shapes.scad` includes `units.scad`; a workspace `units.scad`
    // is what that directive must reach.
    let shape = evaluate_project(&project(
        "include <shapes.scad>\ncube(inch);",
        &[("units.scad", "inch = 3;")],
    ))
    .expect("include");
    assert_inside(&shape, Vec3::new(2.5, 2.5, 2.5));
    assert_outside(&shape, Vec3::new(4.0, 1.0, 1.0));
}

#[test]
fn bundled_libraries_remain_reachable_from_a_project() {
    evaluate_project(&project("include <units.scad>\ncube(inch);", &[])).expect("bundled include");
}

#[test]
fn a_main_file_including_itself_is_a_no_op() {
    let shape = evaluate_project(&project("include <main.scad>\ncube(2);", &[])).expect("self");
    assert_inside(&shape, Vec3::new(1.0, 1.0, 1.0));
    let program =
        parse_resolved_program(&project("include <main.scad>\ncube(2);", &[])).expect("parse");
    assert_eq!(
        program.statements.len(),
        1,
        "main must not splice itself in"
    );
}

#[test]
fn a_cycle_between_workspace_files_terminates() {
    let shape = evaluate_project(&project(
        "include <a.scad>\ncube(a + b);",
        &[
            ("a.scad", "include <b.scad>\na = 1;"),
            ("b.scad", "include <a.scad>\nb = 2;"),
        ],
    ))
    .expect("cycle");
    assert_inside(&shape, Vec3::new(2.5, 2.5, 2.5));
    assert_outside(&shape, Vec3::new(3.5, 1.0, 1.0));
}

#[test]
fn errors_in_an_included_file_name_that_file() {
    let error = evaluate_project(&project(
        "include <broken.scad>\ncube(1);",
        &[("broken.scad", "x = 1;\ny = ;")],
    ))
    .expect_err("broken include")
    .to_string();
    assert!(error.starts_with("broken.scad: "), "{error}");
    let error = evaluate_project(&project(
        "include <broken.scad>\ncube(1);",
        &[("broken.scad", "x = 1;\ny = \"open;")],
    ))
    .expect_err("unterminated string")
    .to_string();
    assert!(error.starts_with("broken.scad: Line 2:"), "{error}");
}

#[test]
fn errors_in_the_main_file_are_unprefixed() {
    let error = evaluate_project(&project("cube(1);\ny = ;", &[("lib.scad", "")]))
        .expect_err("broken main")
        .to_string();
    assert!(error.starts_with("Expected an expression"), "{error}");
    // Lexer errors carry a line number, and it is the main file's.
    let error = evaluate_project(&project("cube(1);\ny = \"open;", &[]))
        .expect_err("unterminated string")
        .to_string();
    assert!(error.starts_with("Line 2:"), "{error}");
}

#[test]
fn a_missing_file_inside_an_included_file_names_the_includer() {
    let error = evaluate_project(&project(
        "include <a.scad>\ncube(1);",
        &[("a.scad", "include <missing.scad>")],
    ))
    .expect_err("missing")
    .to_string();
    assert!(error.starts_with("a.scad: Line 1:"), "{error}");
    assert!(error.contains("missing.scad"), "{error}");
}

#[test]
fn a_missing_main_file_is_an_error() {
    let project = Project {
        main: "main.scad".into(),
        files: BTreeMap::new(),
    };
    let error = evaluate_project(&project).expect_err("no main").to_string();
    assert!(error.contains("main.scad"), "{error}");
}

#[test]
fn used_definitions_see_their_own_files_variables() {
    let shape = evaluate_project(&project(
        "use <lib.scad>\nk = 10;\ncube(f());",
        &[("lib.scad", "k = 3;\nfunction f() = k;")],
    ))
    .expect("use");
    assert_inside(&shape, Vec3::new(2.5, 2.5, 2.5));
    assert_outside(&shape, Vec3::new(4.0, 1.0, 1.0));
}

#[test]
fn used_modules_see_their_own_files_variables() {
    let shape = evaluate_project(&project(
        "use <lib.scad>\nblock();",
        &[("lib.scad", "edge = 4;\nmodule block() cube(edge);")],
    ))
    .expect("use");
    assert_inside(&shape, Vec3::new(3.5, 3.5, 3.5));
    assert_outside(&shape, Vec3::new(4.5, 1.0, 1.0));
}

#[test]
fn used_modules_see_variables_their_file_included() {
    let shape = evaluate_project(&project(
        "use <lib.scad>\ninch_cube();",
        &[(
            "lib.scad",
            "include <units.scad>\nmodule inch_cube() cube(inch);",
        )],
    ))
    .expect("use");
    assert_inside(&shape, Vec3::new(25.0, 25.0, 25.0));
}

#[test]
fn a_used_files_variables_do_not_leak_into_the_caller() {
    let shape = evaluate_project(&project(
        "use <lib.scad>\ncube(is_undef(k) ? 1 : 50);",
        &[("lib.scad", "k = 3;\nfunction f() = k;")],
    ))
    .expect("evaluate");
    assert_outside(&shape, Vec3::new(2.0, 0.5, 0.5));
}

#[test]
fn used_files_contribute_no_geometry() {
    let shape = evaluate_project(&project(
        "use <lib.scad>\ncube(1);",
        &[("lib.scad", "translate([50, 0, 0]) cube(1);")],
    ))
    .expect("use");
    assert_outside(&shape, Vec3::new(50.5, 0.5, 0.5));
}

#[test]
fn the_callers_definitions_take_precedence_over_used_ones() {
    let shape = evaluate_project(&project(
        "use <lib.scad>\nmodule part() cube(5);\npart();",
        &[("lib.scad", "module part() cube(1);")],
    ))
    .expect("use");
    assert_inside(&shape, Vec3::new(4.5, 4.5, 4.5));
}

#[test]
fn use_is_not_transitive() {
    let messages = validate(project(
        "use <a.scad>\ninner();",
        &[
            ("a.scad", "use <b.scad>\nmodule outer() inner();"),
            ("b.scad", "module inner() cube(1);"),
        ],
    ))
    .expect("unknown modules are warnings");
    assert!(
        messages
            .iter()
            .any(|message| message == "WARNING: Ignoring unknown module 'inner'."),
        "{messages:?}"
    );
    // ...while a.scad's own definitions still reach what a.scad used.
    let shape = evaluate_project(&project(
        "use <a.scad>\nouter();",
        &[
            ("a.scad", "use <b.scad>\nmodule outer() inner();"),
            ("b.scad", "module inner() cube(1);"),
        ],
    ))
    .expect("nested use");
    assert_inside(&shape, Vec3::new(0.5, 0.5, 0.5));
}

#[test]
fn the_compile_cache_sees_edits_to_included_files() {
    let small = project("include <p.scad>\ncube(size);", &[("p.scad", "size = 2;")]);
    let large = project("include <p.scad>\ncube(size);", &[("p.scad", "size = 9;")]);
    let first = compile_shared(&small, Quality::Preview, None).expect("small");
    let second = compile_shared(&large, Quality::Preview, None).expect("large");
    assert!((max_x(&first.mesh) - 2.0).abs() < 1e-6);
    assert!((max_x(&second.mesh) - 9.0).abs() < 1e-6);
    let first = compile_parts_shared(&small, Quality::Preview, None).expect("small");
    let second = compile_parts_shared(&large, Quality::Preview, None).expect("large");
    assert!((max_x(&first.mesh) - 2.0).abs() < 1e-6);
    assert!((max_x(&second.mesh) - 9.0).abs() < 1e-6);
}

#[test]
fn a_single_source_still_compiles_unchanged() {
    validate("cube(1);").expect("plain source");
    validate(Project::single("cube(1);")).expect("single project");
}
