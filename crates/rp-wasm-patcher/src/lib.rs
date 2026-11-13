//! Clean-room WebAssembly (`game.wasm`) inspector and Roleplay bridge patcher.
//!
//! Reads the standard WebAssembly `name` custom section to discover internal C++ command
//! symbols and re-exports them as `mp:<Name>` host-callable exports, while injecting a
//! frame tick callback into `GtaThread::Update(int)`.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use wasm_encoder::reencode::{utils, Error, Reencode};
use wasm_encoder::ValType::I64;
use wasmparser::{KnownCustom, Name, Parser, Payload, TypeRef};

pub const HOOK_IMPORT_MODULE: &str = "env";
pub const HOOK_SCRIPT_TICK: &str = "rp_script_tick";
const MOVIE_PRELOAD_SYMBOL: &str = "CMovieMgr::PreloadMovie(char const*, bool)";

/// Core & Roleplay RAGE script commands automatically exported as `mp:<ShortName>`.
/// Because `game.wasm` preserves C++ function names, no hex memory offset reverse-engineering is required!
pub const RP_COMMANDS: &[(&str, &str)] = &[
    // Player & Entity Core
    ("PlayerPedId", "player_commands::CommandPlayerPedId()"),
    ("PlayerId", "player_commands::CommandPlayerId()"),
    ("DoesEntityExist", "entity_commands::CommandDoesEntityExist(int)"),
    ("GetEntityCoords", "entity_commands::CommandGetEntityCoords(int, bool)"),
    ("SetEntityCoordsNoOffset", "entity_commands::CommandSetEntityCoordsNoOffset(int, rage::scrVector const&, bool, bool, bool)"),
    ("GetEntityHeading", "entity_commands::CommandGetEntityHeading(int)"),
    ("SetEntityHeading", "entity_commands::CommandSetEntityHeading(int, float)"),
    ("GetEntityVelocity", "entity_commands::CommandGetEntityVelocity(int)"),
    ("SetEntityVelocity", "entity_commands::CommandSetEntityVelocity(int, rage::scrVector const&)"),
    ("GetEntityRotation", "entity_commands::CommandGetEntityRotation(int, int)"),
    ("SetEntityRotation", "entity_commands::CommandSetEntityRotation(int, rage::scrVector const&, int, bool)"),
    ("GetEntityModel", "entity_commands::CommandGetEntityModel(int)"),
    ("GetEntityHealth", "entity_commands::CommandGetEntityHealth(int)"),
    ("SetEntityHealth", "entity_commands::CommandSetEntityHealth(int, int, int)"),
    ("FreezeEntityPosition", "entity_commands::CommandFreezeEntityPosition(int, bool)"),
    ("SetEntityInvincible", "entity_commands::CommandSetEntityInvincible(int, bool)"),
    ("SetEntityAsMissionEntity", "entity_commands::CommandSetEntityAsMissionEntity(int, bool, bool)"),
    ("DeleteEntity", "entity_commands::CommandDeleteEntity(int&)"),

    // Ped, Customization & Roleplay Animations
    ("CreatePed", "ped_commands::CommandCreatePed(int, int, rage::scrVector const&, float, bool, bool)"),
    ("DeletePed", "ped_commands::CommandDeletePed(int&)"),
    ("SetPedComponentVariation", "ped_commands::CommandSetPedComponentVariation(int, int, int, int, int)"),
    ("SetPedPropIndex", "ped_commands::CommandSetPedPropIndex(int, int, int, int, bool)"),
    ("ClearPedProp", "ped_commands::CommandClearPedProp(int, int)"),
    ("SetPedArmour", "ped_commands::CommandSetPedArmour(int, int)"),
    ("GetPedArmour", "ped_commands::CommandGetPedArmour(int)"),
    ("DoesAnimDictExist", "streaming_commands::DoesAnimDictExist(char const*)"),
    ("RequestAnimDict", "streaming_commands::RequestAnimDict(char const*)"),
    ("HasAnimDictLoaded", "streaming_commands::HasAnimDictLoaded(char const*)"),
    ("TaskPlayAnim", "task_commands::CommandTaskPlayAnim(int, char const*, char const*, float, float, int, int, float, bool, int, bool)"),
    ("ClearPedTasks", "task_commands::CommandClearPedTasks(int)"),
    ("ClearPedTasksImmediately", "task_commands::CommandClearPedTasksImmediately(int)"),

    // Vehicles & RP Police/Garage Features
    ("RequestModel", "streaming_commands::CommandRequestModel(int)"),
    ("HasModelLoaded", "streaming_commands::HasModelLoaded(int)"),
    ("CreateVehicle", "vehicle_commands::CommandCreateVehicle(int, rage::scrVector const&, float, bool, bool, bool)"),
    ("DeleteVehicle", "vehicle_commands::CommandDeleteVehicle(int&)"),
    ("SetPedIntoVehicle", "ped_commands::CommandSetPedIntoVehicle(int, int, int)"),
    ("GetVehiclePedIsIn", "ped_commands::CommandGetVehiclePedIsIn(int, bool)"),
    ("IsPedInAnyVehicle", "ped_commands::CommandIsPedInAnyVehicle(int, bool)"),
    ("SetVehicleEngineOn", "vehicle_commands::CommandSetVehicleEngineOn(int, bool, bool, bool)"),
    ("SetVehicleColour", "vehicle_commands::CommandSetVehicleColour(int, int, int)"),
    ("SetVehicleNumberPlateText", "vehicle_commands::CommandSetVehicleNumberPlateText(int, char const*)"),
    ("SetVehicleFixed", "vehicle_commands::CommandSetVehicleFixed(int)"),
    ("SetVehicleSiren", "vehicle_commands::CommandSetVehicleSiren(int, bool)"),
    ("SetVehicleDoorsLocked", "vehicle_commands::CommandSetVehicleDoorsLocked(int, int)"),

    // Mobile RAM / FPS Optimization (OneSync-style Zero Local AI Density)
    ("SetPedDensityMultiplierThisFrame", "ped_commands::CommandSetPedDensityMultiplierThisFrame(float)"),
    ("SetScenarioPedDensityMultiplierThisFrame", "ped_commands::CommandSetScenarioPedDensityMultiplierThisFrame(float, float)"),
    ("SetVehicleDensityMultiplierThisFrame", "vehicle_commands::CommandSetVehicleDensityMultiplierThisFrame(float)"),
    ("SetRandomVehicleDensityMultiplierThisFrame", "vehicle_commands::CommandSetRandomVehicleDensityMultiplierThisFrame(float)"),
    ("SetParkedVehicleDensityMultiplierThisFrame", "vehicle_commands::CommandSetParkedVehicleDensityMultiplierThisFrame(float)"),

    // Weapons, Environment & World Clock
    ("GiveWeaponToPed", "weapon_commands::CommandGiveWeaponToPed(int, int, int, bool, bool)"),
    ("RemoveAllPedWeapons", "weapon_commands::CommandRemoveAllPedWeapons(int, bool)"),
    ("SetCurrentPedWeapon", "weapon_commands::CommandSetCurrentPedWeapon(int, int, bool)"),
    ("GetSelectedPedWeapon", "weapon_commands::CommandGetSelectedPedWeapon(int)"),
    ("SetClockTime", "clock_commands::CommandSetClockTime(int, int, int)"),
    ("SetWeatherTypeNowPersist", "misc_commands::CommandSetWeatherTypeNowPersist(char const*)"),
];

#[derive(Debug, Default)]
pub struct WasmSymbolReport {
    pub total_named_functions: usize,
    pub tls_base_global: Option<u32>,
    pub matched_commands: Vec<(String, u32)>,
    pub missing_commands: Vec<String>,
    pub discovered_rage_commands: Vec<(u32, String)>,
}

/// Scans a `.wasm` binary's `name` custom section without modifying it.
/// Useful for discovering new GTA V / RAGE C++ command signatures!
pub fn inspect_wasm_symbols(wasm: &[u8]) -> Result<WasmSymbolReport> {
    let mut by_name: HashMap<String, u32> = HashMap::new();
    let mut discovered_rage_commands = Vec::new();
    let mut tls_base_global = None;

    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::CustomSection(c) = payload? {
            if let KnownCustom::Name(names) = c.as_known() {
                for sub in names {
                    match sub? {
                        Name::Function(map) => {
                            for naming in map {
                                let naming = naming?;
                                by_name.entry(naming.name.to_string()).or_insert(naming.index);
                                if naming.name.contains("_commands::Command") {
                                    discovered_rage_commands
                                        .push((naming.index, naming.name.to_string()));
                                }
                            }
                        }
                        Name::Global(map) => {
                            for naming in map {
                                let naming = naming?;
                                if naming.name == "__tls_base" {
                                    tls_base_global = Some(naming.index);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let mut matched_commands = Vec::new();
    let mut missing_commands = Vec::new();
    for (short, cpp_sig) in RP_COMMANDS {
        if let Some(&idx) = by_name.get(*cpp_sig) {
            matched_commands.push(((*short).to_string(), idx));
        } else {
            missing_commands.push((*short).to_string());
        }
    }

    Ok(WasmSymbolReport {
        total_named_functions: by_name.len(),
        tls_base_global,
        matched_commands,
        missing_commands,
        discovered_rage_commands,
    })
}

pub fn is_already_patched(wasm: &[u8]) -> bool {
    Parser::new(0).parse_all(wasm).any(|p| match p {
        Ok(Payload::ImportSection(s)) => s
            .into_imports()
            .any(|i| i.is_ok_and(|imp| imp.name == HOOK_SCRIPT_TICK)),
        _ => false,
    })
}

struct RpPatcher {
    imported_funcs: u32,
    types_count: u32,
    tls_global: u32,
    script_update_func: u32,
    movie_preload_func: Option<u32>,
    exports_to_add: Vec<(String, u32)>,
    next_func_idx: u32,
}

impl Reencode for RpPatcher {
    type Error = anyhow::Error;

    fn function_index(&mut self, f: u32) -> Result<u32, Error<Self::Error>> {
        // Shift internal function indices by +1 because we inject 1 host import (`rp_script_tick`)
        Ok(if f >= self.imported_funcs { f + 1 } else { f })
    }

    fn parse_type_section(
        &mut self,
        types: &mut wasm_encoder::TypeSection,
        section: wasmparser::TypeSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        utils::parse_type_section(self, types, section)?;
        // Signature for `env.rp_script_tick(thread_ptr: i64, tls_base: i64) -> ()`
        types.ty().function(vec![I64, I64], vec![]);
        Ok(())
    }

    fn parse_import_section(
        &mut self,
        imports: &mut wasm_encoder::ImportSection,
        section: wasmparser::ImportSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        utils::parse_import_section(self, imports, section)?;
        imports.import(
            HOOK_IMPORT_MODULE,
            HOOK_SCRIPT_TICK,
            wasm_encoder::EntityType::Function(self.types_count),
        );
        Ok(())
    }

    fn parse_export_section(
        &mut self,
        exports: &mut wasm_encoder::ExportSection,
        section: wasmparser::ExportSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        utils::parse_export_section(self, exports, section)?;
        for (export_name, orig_idx) in std::mem::take(&mut self.exports_to_add) {
            exports.export(&export_name, wasm_encoder::ExportKind::Func, orig_idx + 1);
        }
        Ok(())
    }

    fn parse_function_body(
        &mut self,
        code: &mut wasm_encoder::CodeSection,
        func: wasmparser::FunctionBody<'_>,
    ) -> Result<(), Error<Self::Error>> {
        let current_orig_idx = self.next_func_idx;
        self.next_func_idx += 1;

        // Stub out `CMovieMgr::PreloadMovie` -> `return 0` so stripped `.bik` videos never hang
        if Some(current_orig_idx) == self.movie_preload_func {
            use wasm_encoder::Instruction as I;
            let mut f = self.new_function_with_parsed_locals(&func)?;
            f.instruction(&I::I32Const(0)).instruction(&I::Return);
            let mut reader = func.get_operators_reader()?;
            while !reader.eof() {
                f.instruction(&self.parse_instruction(&mut reader)?);
            }
            code.function(&f);
            return Ok(());
        }

        if current_orig_idx != self.script_update_func {
            return utils::parse_function_body(self, code, func);
        }

        // Inject `rp_script_tick(local.0, global.__tls_base)` at the entry of `GtaThread::Update(int)`
        use wasm_encoder::Instruction as I;
        let mut f = self.new_function_with_parsed_locals(&func)?;
        f.instruction(&I::LocalGet(0));
        f.instruction(&I::GlobalGet(self.tls_global));
        f.instruction(&I::Call(self.imported_funcs));

        let mut reader = func.get_operators_reader()?;
        while !reader.eof() {
            f.instruction(&self.parse_instruction(&mut reader)?);
        }
        code.function(&f);
        Ok(())
    }
}

/// Patches `game.wasm` to inject the Roleplay tick hook and export all discovered RAGE commands.
pub fn patch_game_wasm(wasm: &[u8], extra_commands: &[(&str, &str)]) -> Result<Vec<u8>> {
    if is_already_patched(wasm) {
        bail!("Wasm module is already patched with {HOOK_SCRIPT_TICK}");
    }

    let mut imported_funcs = 0u32;
    let mut types_count = 0u32;
    let mut tls_global = None;
    let mut by_name: HashMap<String, u32> = HashMap::new();

    for payload in Parser::new(0).parse_all(wasm) {
        match payload? {
            Payload::TypeSection(s) => types_count = s.count(),
            Payload::ImportSection(s) => {
                for imp in s.into_imports() {
                    if matches!(imp?.ty, TypeRef::Func(_)) {
                        imported_funcs += 1;
                    }
                }
            }
            Payload::CustomSection(c) => {
                if let KnownCustom::Name(names) = c.as_known() {
                    for sub in names {
                        match sub? {
                            Name::Function(map) => {
                                for n in map {
                                    let n = n?;
                                    by_name.entry(n.name.to_string()).or_insert(n.index);
                                }
                            }
                            Name::Global(map) => {
                                for n in map {
                                    let n = n?;
                                    if n.name == "__tls_base" {
                                        tls_global = Some(n.index);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let script_update_func = *by_name
        .get("GtaThread::Update(int)")
        .context("GtaThread::Update(int) not found in Wasm name section")?;

    let mut exports_to_add = Vec::new();
    if let Some(&malloc_idx) = by_name.get("emscripten_builtin_malloc") {
        exports_to_add.push(("rp:malloc".to_string(), malloc_idx));
    }

    for (short, cpp_sig) in RP_COMMANDS.iter().chain(extra_commands.iter()) {
        if let Some(&idx) = by_name.get(*cpp_sig) {
            exports_to_add.push((format!("mp:{short}"), idx));
        }
    }

    let mut patcher = RpPatcher {
        imported_funcs,
        types_count,
        tls_global: tls_global.unwrap_or(0),
        script_update_func,
        movie_preload_func: by_name.get(MOVIE_PRELOAD_SYMBOL).copied(),
        exports_to_add,
        next_func_idx: imported_funcs,
    };

    let mut out_module = wasm_encoder::Module::new();
    patcher
        .parse_core_module(&mut out_module, Parser::new(0), wasm)
        .map_err(|e| anyhow::anyhow!("Failed to re-encode Wasm module: {e:?}"))?;
    Ok(out_module.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        CodeSection, ExportSection, Function, FunctionSection, GlobalSection, GlobalType,
        ImportSection, Instruction, Module, NameMap, NameSection, TypeSection, ValType,
    };

    #[test]
    fn test_cleanroom_wasm_patcher_exports_commands_and_hooks_tick() {
        // Build a synthetic mini-Wasm module simulating C++ symbols in Custom Name Section
        let mut module = Module::new();

        let mut types = TypeSection::new();
        types.ty().function(vec![ValType::I64], vec![ValType::I32]); // func 0: GtaThread::Update(int)
        types.ty().function(vec![], vec![ValType::I32]); // func 1: PlayerPedId()
        module.section(&types);

        let imports = ImportSection::new();
        module.section(&imports);

        let mut funcs = FunctionSection::new();
        funcs.function(0);
        funcs.function(1);
        module.section(&funcs);

        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType {
                val_type: ValType::I64,
                mutable: false,
                shared: false,
            },
            &wasm_encoder::ConstExpr::i64_const(4096),
        );
        module.section(&globals);

        let exports = ExportSection::new();
        module.section(&exports);

        let mut code = CodeSection::new();
        let mut f0 = Function::new(vec![]);
        f0.instruction(&Instruction::I32Const(1)).instruction(&Instruction::End);
        code.function(&f0);

        let mut f1 = Function::new(vec![]);
        f1.instruction(&Instruction::I32Const(42)).instruction(&Instruction::End);
        code.function(&f1);
        module.section(&code);

        let mut names = NameSection::new();
        let mut func_names = NameMap::new();
        func_names.append(0, "GtaThread::Update(int)");
        func_names.append(1, "player_commands::CommandPlayerPedId()");
        names.functions(&func_names);

        let mut global_names = NameMap::new();
        global_names.append(0, "__tls_base");
        names.globals(&global_names);
        module.section(&names);

        let raw_wasm = module.finish();
        let report = inspect_wasm_symbols(&raw_wasm).expect("inspect ok");
        assert_eq!(report.matched_commands.len(), 1);
        assert_eq!(report.matched_commands[0].0, "PlayerPedId");

        let patched = patch_game_wasm(&raw_wasm, &[]).expect("patch ok");
        assert!(is_already_patched(&patched));

        // Verify `mp:PlayerPedId` is now exported in the patched module
        let has_mp_export = Parser::new(0).parse_all(&patched).any(|p| match p {
            Ok(Payload::ExportSection(s)) => s
                .into_iter()
                .any(|e| e.is_ok_and(|ex| ex.name == "mp:PlayerPedId")),
            _ => false,
        });
        assert!(has_mp_export, "mp:PlayerPedId should be exported");
    }
}

