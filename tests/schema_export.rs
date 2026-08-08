use orca_slicer_api::contract::{ScopeValue, parse_contract};
use std::{fs, process::Command};

#[test]
fn schema_export_is_deterministic_and_fail_closed() {
    let root = std::env::temp_dir().join(format!("orca-schema-export-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let print_config = root.join("PrintConfig.cpp");
    let print_config_header = root.join("PrintConfig.hpp");
    let constants = root.join("PrintConfigConstants.hpp");
    let tab_source = root.join("Tab.cpp");
    let first = root.join("first.json");
    let second = root.join("second.json");
    fs::write(
        &print_config,
        r#"
static t_config_enum_values s_keys_map_QualityMode {
    { "draft", QualityMode::Draft },
    { "fine", QualityMode::Fine }
};

def = this->add("layer_height", coFloat);
def->label = L("Layer height");
def->tooltip = L("Slicing height");
def->sidetext = L("mm");
def->min = 0;
def->mode = comAdvanced;
def->set_default_value(new ConfigOptionFloat(INITIAL_LAYER_HEIGHT));

auto support_interface_top_layers = def = this->add("support_interface_top_layers", coInt);
def->label = L("Top layers");
def->enum_values.emplace_back("0");
def->enum_values.emplace_back("1");
def->set_default_value(new ConfigOptionInt(0));

auto support_interface_bottom_layers = def = this->add("support_interface_bottom_layers", coInt);
def->label = L("Bottom layers");
def->enum_values.emplace_back("-1");
append(def->enum_values, support_interface_top_layers->enum_values);
def->enum_values.emplace_back("5");
def->set_default_value(new ConfigOptionInt(0));

def = this->add("quality_mode", coEnum);
def->label = L("Quality mode");
def->tooltip = L("Select quality");
def->enum_keys_map = &ConfigOptionEnum<QualityMode>::get_enum_values();
def->enum_values.emplace_back("draft");
def->enum_values.emplace_back("fine");
def->set_default_value(new ConfigOptionEnum<QualityMode>(QualityMode::Draft));
"#,
    )
    .unwrap();
    fs::write(
        &print_config_header,
        r#"
PRINT_CONFIG_CLASS_DEFINE(
    PrintObjectConfig,
    ((ConfigOptionFloat, layer_height))
)
PRINT_CONFIG_CLASS_DEFINE(
    PrintRegionConfig,
    ((ConfigOptionFloat, other_option))
    ((ConfigOptionInt, support_interface_top_layers))
    ((ConfigOptionInt, support_interface_bottom_layers))
)
PRINT_CONFIG_CLASS_DERIVED_DEFINE(FullPrintConfig, (PrintObjectConfig))
"#,
    )
    .unwrap();
    fs::write(&constants, "#define INITIAL_LAYER_HEIGHT 0.2\n").unwrap();
    fs::write(
        &tab_source,
        r#"
void TabPrint::build()
{
    auto page = add_options_page(L("Quality"), "quality");
    auto optgroup = page->new_optgroup(L("Layers"), "layers");
    optgroup->append_single_option_line("layer_height");
    optgroup->append_single_option_line("support_interface_top_layers");
    optgroup->append_single_option_line("support_interface_bottom_layers");
    optgroup->append_single_option_line("quality_mode");
}
void TabPrint::reload_config()
{
}
"#,
    )
    .unwrap();

    for output in [&first, &second] {
        let status = Command::new(env!("CARGO_BIN_EXE_schema-export"))
            .args([
                "--print-config-source",
                print_config.to_str().unwrap(),
                "--print-config-header",
                print_config_header.to_str().unwrap(),
                "--constants-source",
                constants.to_str().unwrap(),
                "--tab-source",
                tab_source.to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
    }

    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    let contract = parse_contract(&fs::read(&first).unwrap()).unwrap();
    assert!(contract.capabilities.process_schema);
    assert!(!contract.capabilities.model_state);
    assert!(contract.capabilities.progress);
    assert!(!contract.capabilities.cancel);
    assert_eq!(contract.process_schema.pages[0].groups[0].options.len(), 4);
    assert_eq!(contract.process_schema.samples["layer_height"], 0.2);
    assert_eq!(contract.process_schema.samples["quality_mode"], "draft");
    assert_eq!(contract.process_schema.options[1]["choices"], serde_json::json!([0, 1]));
    assert_eq!(contract.process_schema.options[2]["choices"], serde_json::json!([-1, 0, 1, 5]));
    assert_eq!(
        contract.process_schema.scopes["layer_height"],
        ScopeValue::Many(vec!["global".into(), "object".into()])
    );
    assert_eq!(contract.process_schema.scopes["quality_mode"], ScopeValue::One("global".into()));
    assert_eq!(contract.process_schema.options[3]["choices"], serde_json::json!(["draft", "fine"]));
    assert_eq!(contract.image_identity.digest, format!("sha256:{}", "0".repeat(64)));
    fs::remove_dir_all(root).unwrap();
}
