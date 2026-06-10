use std::path::PathBuf;

use ramparils::params::ParamSpace;

fn parameter_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/llm2smt/params-llm2smt.txt")
}

#[test]
fn parse_llm2smt_parameter_space() {
    let space = ParamSpace::from_file(parameter_file().to_str().unwrap()).unwrap();
    assert_eq!(space.params.len(), 12);
    assert!(space.forbidden.is_empty());

    let default = space.default_config();
    let active: Vec<&str> = space
        .active_params(&default)
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect();
    assert!(!active.contains(&"nnf_memo"));

    let mut nnf_config = default;
    nnf_config.insert("nnf".to_string(), "true".to_string());
    let active: Vec<&str> = space
        .active_params(&nnf_config)
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect();
    assert!(active.contains(&"nnf_memo"));
}
