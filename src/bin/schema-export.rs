use orca_slicer_api::{
    contract::{
        CONTRACT_VERSION, Capabilities, Contract, Engine, ImageIdentity, ORCA_COMMIT, ORCA_VERSION,
        compact_json, process_hash,
    },
    schema::{self, OrcaSources, build_process_schema},
};
use std::{env, fs, path::PathBuf, process::exit};

fn argument(args: &[String], name: &str) -> Option<PathBuf> {
    args.windows(2).find(|pair| pair[0] == name).map(|pair| PathBuf::from(&pair[1]))
}

fn run() -> Result<(), String> {
    let args = env::args().collect::<Vec<_>>();
    let usage = "usage: schema-export --print-config-source PrintConfig.cpp --print-config-header PrintConfig.hpp --constants-source PrintConfigConstants.hpp --tab-source Tab.cpp --output CONTRACT.json";
    let print_config_path = argument(&args, "--print-config-source").ok_or(usage)?;
    let print_config_header_path = argument(&args, "--print-config-header").ok_or(usage)?;
    let constants_path = argument(&args, "--constants-source").ok_or(usage)?;
    let tab_source_path = argument(&args, "--tab-source").ok_or(usage)?;
    let output = argument(&args, "--output").ok_or(usage)?;
    let print_config = fs::read_to_string(print_config_path).map_err(|error| error.to_string())?;
    let print_config_header =
        fs::read_to_string(print_config_header_path).map_err(|error| error.to_string())?;
    let constants = fs::read_to_string(constants_path).map_err(|error| error.to_string())?;
    let tab_source = fs::read_to_string(tab_source_path).map_err(|error| error.to_string())?;
    let process_schema = build_process_schema(
        OrcaSources {
            print_config: &print_config,
            print_config_header: &print_config_header,
            constants: &constants,
        },
        &tab_source,
    )?;
    let mut contract = Contract {
        contract_version: CONTRACT_VERSION.into(),
        engine: Engine {
            name: "OrcaSlicer".into(),
            version: ORCA_VERSION.into(),
            commit: ORCA_COMMIT.into(),
        },
        image_identity: ImageIdentity { digest: format!("sha256:{}", "0".repeat(64)) },
        schema_hash: String::new(),
        capabilities: Capabilities {
            process_schema: true,
            model_state: false,
            progress: true,
            cancel: false,
        },
        supported_scopes: vec!["global".into(), "object".into()],
        process_schema,
    };
    contract.schema_hash = process_hash(&contract).map_err(|error| error.to_string())?;
    schema::validate(&contract)?;
    let value = serde_json::to_value(contract).map_err(|error| error.to_string())?;
    fs::write(output, compact_json(&value).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("schema export failed: {error}");
        exit(1);
    }
}
