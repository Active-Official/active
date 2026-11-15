use active_core::parse;

#[test]
fn debug_ast2() {
    let src = r#"ye=o[o]yye=o:[n]Iy=o[o]yye=o:e=ov"#;
    let m = parse(src, "dbg").expect("parse");
    for (i, node) in m.arena.iter().enumerate() {
        println!("{i}: {node:?}");
    }
}
