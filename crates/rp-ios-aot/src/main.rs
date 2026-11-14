//! Headless cross-platform Cranelift AOT precompiler for stock non-JIT iOS (`aarch64-apple-ios`).
//!
//! Runs on any Linux/macOS/Windows build server (e.g. 6-vCPU Ubuntu VPS) to:
//! 1. Inject all 9 Wasm host hooks (`muguet_script`, `muguet_pool` 2.5 GB iOS RAM cap,
//!    `muguet_opus_frame`, `muguet_resource_map`, `muguet_txd`, `muguet_text`, `muguet_cash`,
//!    `muguet_explosion`, `muguet_timewarp`, `SCENARIO_CHECK`, `MOVIE_PRELOAD`).
//! 2. Export all 206 `mp:<Command>` GTA V RAGE natives for 3D sync + JavaScript Roleplay RPCs.
//! 3. Cross-compile `game.wasm` ahead-of-time into `game.cwasm` (`aarch64-apple-ios` + ARMv8.1 LSE)
//!    ready to be embedded into the `.ipa`'s `__GAME,__code` Mach-O segment.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::time::Instant;
use wasm_encoder::reencode::{utils, Error, Reencode};
use wasm_encoder::ValType::I64;
use wasmparser::{KnownCustom, Name, Parser, Payload, TypeRef};
use wasmtime::{Config, Engine, OptLevel};

pub const MISSING_TEXT: &[(&str, &str)] = &[
    (
        "HELP_DCK",
        "~s~Hold ~INPUT_VEH_DUCK~ to duck when sitting inside a vehicle.",
    ),
    (
        "HELP_KM",
        "~s~Press ~INPUT_CONTEXT~ to use the camera in your glasses, then ~INPUT_ATTACK~ to take a photo.",
    ),
];

pub const COMMANDS: &[(&str, &str)] = &[
    ("PlayerPedId", "player_commands::CommandPlayerPedId()"),
    ("IsPauseMenuActive", "hud_commands::CommandIsPauseMenuActive()"),
    ("SetControlValueNextFrame", "pad_commands::CommandSetControlValueNextFrame(int, int, float)"),
    ("GetVehicleClass", "vehicle_commands::CommandGetVehicleClass(int)"),
    ("DetachVehicleFromAnyTowTruck", "vehicle_commands::CommandDetachVehicleFromAnyTowTruck(int)"),
    ("GetEntityAttachedToTowTruck", "vehicle_commands::CommandGetEntityAttachedToTowTruck(int)"),
    ("IsVehicleAttachedToTrailer", "vehicle_commands::CommandIsVehicleAttachedToTrailer(int)"),
    ("DetachVehicleFromTrailer", "vehicle_commands::CommandDetachVehicleFromTrailer(int)"),
    ("IsVehicleAConvertible", "vehicle_commands::CommandIsVehicleAConvertible(int, bool)"),
    ("DoesCargobobHavePickUpRope", "vehicle_commands::CommandDoesCargobobHavePickUpRope(int)"),
    ("CreatePickUpRopeForCargobob", "vehicle_commands::CommandCreatePickUpRopeForCargobob(int, int)"),
    ("RemovePickUpRopeForCargobob", "vehicle_commands::CommandRemovePickUpRopeForCargobob(int)"),
    ("GetEntityCoords", "entity_commands::CommandGetEntityCoords(int, bool)"),
    ("GetEntityHeading", "entity_commands::CommandGetEntityHeading(int)"),
    ("GetEntityVelocity", "entity_commands::CommandGetEntityVelocity(int)"),
    ("GetEntityRotation", "entity_commands::CommandGetEntityRotation(int, int)"),
    ("GetEntityModel", "entity_commands::CommandGetEntityModel(int)"),
    ("GetEntityHealth", "entity_commands::CommandGetEntityHealth(int)"),
    ("SetEntityHealth", "entity_commands::CommandSetEntityHealth(int, int, int)"),
    ("FreezeEntityPosition", "entity_commands::CommandFreezeEntityPosition(int, bool)"),
    ("DoesEntityExist", "entity_commands::CommandDoesEntityExist(int)"),
    ("IsEntityAMissionEntity", "entity_commands::CommandIsEntityAMissionEntity(int)"),
    ("SetEntityCoordsNoOffset", "entity_commands::CommandSetEntityCoordsNoOffset(int, rage::scrVector const&, bool, bool, bool)"),
    ("SetEntityHeading", "entity_commands::CommandSetEntityHeading(int, float)"),
    ("SetEntityRotation", "entity_commands::CommandSetEntityRotation(int, rage::scrVector const&, int, bool)"),
    ("SetEntityVelocity", "entity_commands::CommandSetEntityVelocity(int, rage::scrVector const&)"),
    ("SetEntityInvincible", "entity_commands::CommandSetEntityInvincible(int, bool)"),
    ("SetEntityAsMissionEntity", "entity_commands::CommandSetEntityAsMissionEntity(int, bool, bool)"),
    ("DeleteEntity", "entity_commands::CommandDeleteEntity(int&)"),
    ("CreatePed", "ped_commands::CommandCreatePed(int, int, rage::scrVector const&, float, bool, bool)"),
    ("DeletePed", "ped_commands::CommandDeletePed(int&)"),
    ("GetVehiclePedIsIn", "ped_commands::CommandGetVehiclePedIsIn(int, bool)"),
    ("IsPedInAnyVehicle", "ped_commands::CommandIsPedInAnyVehicle(int, bool)"),
    ("IsPedDeadOrDying", "ped_commands::CommandIsPedDeadOrDying(int, bool)"),
    ("IsPedShooting", "ped_commands::CommandIsPedShooting(int)"),
    ("GetPedArmour", "ped_commands::CommandGetPedArmour(int)"),
    ("GetPedBoneCoords", "ped_commands::CommandGetPedBoneCoords(int, int, rage::scrVector const&)"),
    ("SetPedIntoVehicle", "ped_commands::CommandSetPedIntoVehicle(int, int, int)"),
    ("SetBlockingOfNonTemporaryEvents", "ped_commands::CommandSetBlockingOfNonTemporaryEvents(int, bool)"),
    ("SetPedCanRagdoll", "ped_commands::CommandSetPedCanRagdoll(int, bool)"),
    ("SetPedKeepTask", "ped_commands::CommandSetPedKeepTask(int, bool)"),
    ("SetPedCanBeDraggedOut", "ped_commands::CommandSetPedCanBeDraggedOut(int, bool)"),
    ("SetPedDefaultComponentVariation", "ped_commands::CommandSetPedDefaultComponentVariation(int)"),
    ("GetPedDrawableVariation", "ped_commands::CommandGetPedDrawableVariation(int, int)"),
    ("GetPedTextureVariation", "ped_commands::CommandGetPedTextureVariation(int, int)"),
    ("GetPedPaletteVariation", "ped_commands::CommandGetPedPaletteVariation(int, int)"),
    ("SetPedComponentVariation", "ped_commands::CommandSetPedComponentVariation(int, int, int, int, int)"),
    ("GetPedPropIndex", "ped_commands::CommandGetPedPropIndex(int, int)"),
    ("GetPropTextureIndex", "ped_commands::CommandGetPropTextureIndex(int, int)"),
    ("SetPedPropIndex", "ped_commands::CommandSetPedPropIndex(int, int, int, int, bool)"),
    ("ClearPedProp", "ped_commands::CommandClearPedProp(int, int)"),
    ("GetNearbyVehicles", "ped_commands::CommandGetNearbyVehicles(int, int&)"),
    ("GetNearbyPeds", "ped_commands::CommandGetNearbyPeds(int, int&, int)"),
    ("SetPedDensityMultiplierThisFrame", "ped_commands::CommandSetPedDensityMultiplierThisFrame(float)"),
    ("SetScenarioPedDensityMultiplierThisFrame", "ped_commands::CommandSetScenarioPedDensityMultiplierThisFrame(float, float)"),
    ("CreateVehicle", "vehicle_commands::CommandCreateVehicle(int, rage::scrVector const&, float, bool, bool, bool)"),
    ("DeleteVehicle", "vehicle_commands::CommandDeleteVehicle(int&)"),
    ("SetVehicleEngineOn", "vehicle_commands::CommandSetVehicleEngineOn(int, bool, bool, bool)"),
    ("GetPedInVehicleSeat", "vehicle_commands::CommandGetPedInVehicleSeat(int, int, bool)"),
    ("IsVehicleSeatFree", "vehicle_commands::CommandIsVehicleSeatFree(int, int, bool)"),
    ("GetVehicleColours", "vehicle_commands::CommandGetVehicleColours(int, int&, int&)"),
    ("SetVehicleColour", "vehicle_commands::CommandSetVehicleColour(int, int, int)"),
    ("SetVehicleDensityMultiplierThisFrame", "vehicle_commands::CommandSetVehicleDensityMultiplierThisFrame(float)"),
    ("SetRandomVehicleDensityMultiplierThisFrame", "vehicle_commands::CommandSetRandomVehicleDensityMultiplierThisFrame(float)"),
    ("SetParkedVehicleDensityMultiplierThisFrame", "vehicle_commands::CommandSetParkedVehicleDensityMultiplierThisFrame(float)"),
    ("ClearAreaOfVehicles", "misc_commands::CommandClearAreaOfVehicles(rage::scrVector const&, float, bool, bool, bool, bool, bool, bool, bool)"),
    ("ClearAreaOfPeds", "misc_commands::CommandClearAreaOfPeds(rage::scrVector const&, float, bool)"),
    ("FireSingleBulletIgnoreEntity", "misc_commands::CommandFireSingleBulletIgnoreEntity(rage::scrVector const&, rage::scrVector const&, int, bool, int, int, bool, bool, float, int, int)"),
    ("RequestWeaponAsset", "weapon_commands::RequestWeaponAsset(int, int, int)"),
    ("HasWeaponAssetLoaded", "weapon_commands::HasWeaponAssetLoaded(int)"),
    ("GetPlayerInvincible", "player_commands::CommandGetPlayerInvincible(int)"),
    ("GetEntityCanBeDamaged", "entity_commands::CommandGetEntityCanBeDamaged(int)"),
    ("GetVehicleEngineHealth", "vehicle_commands::CommandGetVehicleEngineHealth(int)"),
    ("SetVehicleEngineHealth", "vehicle_commands::CommandSetVehicleEngineHealth(int, float)"),
    ("GetVehicleBodyHealth", "vehicle_commands::CommandGetVehicleBodyHealth(int)"),
    ("SetVehicleBodyHealth", "vehicle_commands::CommandSetVehicleBodyHealth(int, float)"),
    ("GetVehiclePetrolTankHealth", "vehicle_commands::CommandGetVehiclePetrolTankHealth(int)"),
    ("SetVehiclePetrolTankHealth", "vehicle_commands::CommandSetVehiclePetrolTankHealth(int, float)"),
    ("IsVehicleTyreBurst", "vehicle_commands::CommandIsVehicleTyreBurst(int, int, bool)"),
    ("SetVehicleTyreBurst", "vehicle_commands::CommandSetVehicleTyreBurst(int, int, bool, float)"),
    ("IsVehicleWindowIntact", "vehicle_commands::CommandIsVehicleWindowIntact(int, int)"),
    ("SmashVehicleWindow", "vehicle_commands::CommandSmashVehicleWindow(int, int)"),
    ("ExplodeVehicle", "vehicle_commands::CommandExplodeVehicle(int, bool, bool)"),
    ("SetVehicleFixed", "vehicle_commands::CommandSetVehicleFixed(int)"),
    ("SetVehicleDeformationFixed", "vehicle_commands::CommandSetVehicleDeformationFixed(int)"),
    ("IsEntityDead", "entity_commands::CommandIsEntityDead(int, bool)"),
    ("IsEntityAPed", "entity_commands::CommandIsEntityAPed(int)"),
    ("IsEntityOnScreen", "entity_commands::CommandIsEntityOnScreen(int)"),
    ("SetEntityAsNoLongerNeeded", "entity_commands::CommandSetEntityAsNoLongerNeeded(int&)"),
    ("SetDrawOrigin", "graphics_commands::CommandSetDrawOrigin(rage::scrVector const&, bool)"),
    ("ClearDrawOrigin", "graphics_commands::CommandClearDrawOrigin()"),
    ("BeginTextCommandDisplayText", "hud_commands::CommandBeginTextCommandDisplayText(char const*)"),
    ("AddTextComponentSubStringPlayerName", "hud_commands::CommandAddTextComponentSubStringPlayerName(char const*)"),
    ("EndTextCommandDisplayText", "hud_commands::CommandEndTextCommandDisplayText(float, float, int)"),
    ("SetTextScale", "hud_commands::CommandSetTextScale(float, float)"),
    ("SetTextColour", "hud_commands::CommandSetTextColour(int, int, int, int)"),
    ("SetTextCentre", "hud_commands::CommandSetTextCentre(bool)"),
    ("SetTextFont", "hud_commands::CommandSetTextFont(int)"),
    ("SetTextOutline", "hud_commands::CommandSetTextOutline()"),
    ("DoesScriptExist", "script_commands::DoesScriptExist(char const*)"),
    ("AddRelationshipGroup", "ped_commands::CommandAddRelationshipGroup(char const*, int&)"),
    ("DoesRelationshipGroupExist", "ped_commands::CommandDoesRelationshipGroupExist(int)"),
    ("SetRelationshipBetweenGroups", "ped_commands::CommandSetRelationshipBetweenGroups(int, int, int)"),
    ("SetRelationshipGroupAffectsWantedLevel", "ped_commands::CommandSetRelationshipGroupAffectsWantedLevel(int, bool)"),
    ("GetPedRelationshipGroupHash", "ped_commands::CommandGetPedRelationshipGroupHash(int)"),
    ("SetPedRelationshipGroupHash", "ped_commands::CommandSetPedRelationshipGroupHash(int, int)"),
    ("SetPedCanBeTargetted", "ped_commands::CommandSetPedCanBeTargetted(int, bool)"),
    ("SetCanAttackFriendly", "ped_commands::CommandSetCanAttackFriendly(int, bool, bool)"),
    ("PlayerId", "player_commands::CommandPlayerId()"),
    ("IsPlayerFreeAiming", "player_commands::CommandIsPlayerFreeAiming(int)"),
    ("GetGameplayCamCoord", "camera_commands::CommandGetGameplayCamCoord()"),
    ("GetGameplayCamRot", "camera_commands::CommandGetGameplayCamRot(int)"),
    ("TaskGoToCoordWhileAimingAtCoord", "task_commands::CommandTaskGoToCoordWhileAimingAtCoord(int, rage::scrVector const&, rage::scrVector const&, float, bool, float, float, bool, int, bool, int)"),
    ("GetVehicleExtraColours", "vehicle_commands::CommandGetVehicleExtraColours(int, int&, int&)"),
    ("SetVehicleExtraColours", "vehicle_commands::CommandSetVehicleExtraColours(int, int, int)"),
    ("GetVehicleLivery", "vehicle_commands::CommandGetVehicleLivery(int)"),
    ("SetVehicleLivery", "vehicle_commands::CommandSetVehicleLivery(int, int)"),
    ("GetVehicleWindowTint", "vehicle_commands::CommandGetVehicleWindowTint(int)"),
    ("SetVehicleWindowTint", "vehicle_commands::CommandSetVehicleWindowTint(int, int)"),
    ("GetVehicleWheelType", "vehicle_commands::CommandGetVehicleWheelType(int)"),
    ("SetVehicleWheelType", "vehicle_commands::CommandSetVehicleWheelType(int, int)"),
    ("SetVehicleModKit", "vehicle_commands::CommandSetVehicleModKit(int, int)"),
    ("GetVehicleMod", "vehicle_commands::CommandGetVehicleMod(int, int)"),
    ("SetVehicleMod", "vehicle_commands::CommandSetVehicleMod(int, int, int, bool)"),
    ("IsToggleModOn", "vehicle_commands::CommandIsToggleModOn(int, int)"),
    ("ToggleVehicleMod", "vehicle_commands::CommandToggleVehicleMod(int, int, bool)"),
    ("GetVehicleNumberPlateText", "vehicle_commands::CommandGetVehicleNumberPlateText(int)"),
    ("SetVehicleNumberPlateText", "vehicle_commands::CommandSetVehicleNumberPlateText(int, char const*)"),
    ("GetPlayerRadioStationIndex", "audio_commands::CommandGetPlayerRadioStationIndex()"),
    ("RetuneRadioToStationIndex", "audio_commands::CommandRetuneRadioToStationIndex(int)"),
    ("GetBaseFromGuid", "rage::fwScriptGuid::GetBaseFromGuid(int)"),
    ("AddOwnedExplosion", "fire_commands::CommandAddOwnedExplosion(int, rage::scrVector const&, int, float, bool, bool, float)"),
    ("AddExplosion", "fire_commands::CommandAddExplosion(rage::scrVector const&, int, float, bool, bool, float, bool)"),
    ("SetEntityProofs", "entity_commands::CommandSetEntityProofs(int, bool, bool, bool, bool, bool, bool, bool, bool)"),
    ("SetPedArmour", "ped_commands::CommandSetPedArmour(int, int)"),
    ("SetTimeScale", "misc_commands::CommandSetTimeScale(float)"),
    ("IsControlPressed", "pad_commands::CommandIsControlPressed(int, int)"),
    ("SetClockTime", "clock_commands::CommandSetClockTime(int, int, int)"),
    ("GetClockHours", "clock_commands::CommandGetClockHours()"),
    ("GetClockMinutes", "clock_commands::CommandGetClockMinutes()"),
    ("GetClockSeconds", "clock_commands::CommandGetClockSeconds()"),
    ("GetNextWeatherTypeHashName", "misc_commands::CommandGetNextWeatherTypeHashName()"),
    ("SetWeatherTypeNowPersist", "misc_commands::CommandSetWeatherTypeNowPersist(char const*)"),
    ("HasEntityBeenDamagedByEntity", "entity_commands::CommandHasEntityBeenDamagedByEntity(int, int, bool)"),
    ("ClearEntityLastDamageEntity", "entity_commands::CommandClearEntityLastDamageEntity(int)"),
    ("GetEntitySpeed", "entity_commands::CommandGetEntitySpeed(int)"),
    ("GetMeleeTargetForPed", "ped_commands::CommandGetMeleeTargetForPed(int)"),
    ("DoesAnimDictExist", "streaming_commands::DoesAnimDictExist(char const*)"),
    ("HasAnimDictLoaded", "streaming_commands::HasAnimDictLoaded(char const*)"),
    ("RequestAnimDict", "streaming_commands::RequestAnimDict(char const*)"),
    ("GetAnimDuration", "entity_commands::CommandGetAnimDuration(char const*, char const*)"),
    ("TaskPlayAnim", "task_commands::CommandTaskPlayAnim(int, char const*, char const*, float, float, int, int, float, bool, int, bool)"),
    ("GetWeatherState", "misc_commands::CommandGetWeatherState(int&, int&, float&)"),
    ("SetWeatherState", "misc_commands::CommandSetWeatherState(int, int, float)"),
    ("ClearWeatherTypePersist", "misc_commands::CommandClearWeatherTypePersist()"),
    ("ClearPedTasks", "task_commands::CommandClearPedTasks(int)"),
    ("SetEntityAngularVelocity", "entity_commands::CommandSetEntityAngularVelocity(int, rage::scrVector const&)"),
    ("GetPedCauseOfDeath", "ped_commands::CommandGetPedCauseOfDeath(int)"),
    ("BeginTheFeedPost", "hud_commands::CommandBeginTheFeedPost(char const*)"),
    ("EndTheFeedPostTicker", "hud_commands::CommandEndTheFeedPostTicker(bool, bool)"),
    ("IsPlayerPressingHorn", "player_commands::CommandIsPlayerPressingHorn(int)"),
    ("IsPedRagdoll", "ped_commands::CommandIsPedRagdoll(int)"),
    ("TaskLeaveVehicle", "task_commands::CommandTaskLeaveVehicle(int, int, int)"),
    ("GetVehiclePedIsTryingToEnter", "ped_commands::CommandGetVehiclePedIsTryingToEnter(int)"),
    ("GetSeatPedIsTryingToEnter", "ped_commands::CommandGetSeatPedIsTryingToEnter(int)"),
    ("GetVehicleLightsState", "vehicle_commands::CommandGetVehicleLightsState(int, int&, int&)"),
    ("SetVehicleLights", "vehicle_commands::CommandSetVehicleLights(int, int)"),
    ("SetVehicleFullBeam", "vehicle_commands::CommandSetVehicleFullBeam(int, bool)"),
    ("IsHornActive", "audio_commands::CommandIsHornActive(int)"),
    ("SoundCarHorn", "vehicle_commands::CommandSoundCarHorn(int, int, int, bool)"),
    ("IsVehicleSirenOn", "vehicle_commands::CommandIsVehicleSirenOn(int)"),
    ("SetVehicleSiren", "vehicle_commands::CommandSetVehicleSiren(int, bool)"),
    ("GetVehicleDoorAngleRatio", "vehicle_commands::CommandGetVehicleDoorAngleRatio(int, int)"),
    ("SetVehicleDoorOpen", "vehicle_commands::CommandSetVehicleDoorOpen(int, int, bool, bool)"),
    ("SetVehicleDoorShut", "vehicle_commands::CommandSetVehicleDoorShut(int, int, bool)"),
    ("DrawRect", "graphics_commands::CommandDrawRect(float, float, float, float, int, int, int, int, bool)"),
    ("PlaySoundFrontend", "audio_commands::CommandPlaySoundFrontend(int, char const*, char const*, bool)"),
    ("GetInteriorFromEntity", "interior_commands::CommandGetInteriorFromEntity(int)"),
    ("GetPedSourceOfDeath", "ped_commands::CommandGetPedSourceOfDeath(int)"),
    ("IsPedArmed", "weapon_commands::CommandIsPedArmed(int, int)"),
    ("IsPedSwimming", "ped_commands::CommandIsPedSwimming(int)"),
    ("IsPedSwimmingUnderWater", "ped_commands::CommandIsPedSwimmingUnderWater(int)"),
    ("GetWeaponDamage", "weapon_commands::CommandGetWeaponDamage(int, int)"),
    ("GetWeaponDamageType", "weapon_commands::CommandGetWeaponDamageType(int)"),
    ("SetPedToRagdoll", "ped_commands::CommandSetPedToRagdoll(int, int, int, int, bool, bool, bool)"),
    ("ShakeGameplayCam", "camera_commands::CommandShakeGameplayCam(char const*, float)"),
    ("SetPedShootsAtCoord", "ped_commands::CommandSetPedShootsAtCoord(int, rage::scrVector const&, bool)"),
    ("SetPedMaxHealth", "ped_commands::CommandSetPedMaxHealth(int, int)"),
    ("GetSelectedPedWeapon", "weapon_commands::CommandGetSelectedPedWeapon(int)"),
    ("GetWeaponTypeGroup", "weapon_commands::CommandGetWeaponTypeGroup(int)"),
    ("GiveWeaponToPed", "weapon_commands::CommandGiveWeaponToPed(int, int, int, bool, bool)"),
    ("SetCurrentPedWeapon", "weapon_commands::CommandSetCurrentPedWeapon(int, int, bool)"),
    ("SetPedInfiniteAmmoClip", "weapon_commands::CommandSetPedInfiniteAmmoClip(int, bool)"),
    ("GetPedLastWeaponImpactCoord", "weapon_commands::CommandGetPedLastWeaponImpactCoord(int, rage::Vector3&)"),
    ("RequestModel", "streaming_commands::CommandRequestModel(int)"),
    ("HasModelLoaded", "streaming_commands::HasModelLoaded(int)"),
    ("TaskGoStraightToCoord", "task_commands::CommandTaskGoStraightToCoord(int, rage::scrVector const&, float, int, float, float)"),
    ("TaskAimGunAtCoord", "task_commands::CommandTaskAimGunAtCoord(int, rage::scrVector const&, int, bool, bool)"),
    ("TaskEnterVehicle", "task_commands::CommandTaskEnterVehicle(int, int, int, int, float, int, char const*)"),
    ("ClearPedTasksImmediately", "task_commands::CommandClearPedTasksImmediately(int)"),
    ("AddBlipForEntity", "hud_commands::AddBlipForEntity(int)"),
    ("RemoveBlip", "hud_commands::CommandRemoveBlip(int&)"),
    ("ShowHeadingIndicatorOnBlip", "hud_commands::CommandShowHeadingIndicatorOnBlip(int, bool)"),
    ("RemoveAllPedWeapons", "weapon_commands::CommandRemoveAllPedWeapons(int, bool)"),
    ("SetVehicleDoorsLocked", "vehicle_commands::CommandSetVehicleDoorsLocked(int, int)"),
];

#[derive(Clone, Copy, PartialEq)]
enum Arg {
    Local0,
    TlsBase,
    Local1Replaced,
    MissingText,
    TxdLoaded,
    PoolGrow,
    CashWrite,
    OpusFrame,
    ResourceMap,
}

const HOOKS: &[(&str, &str, &[Arg])] = &[
    ("muguet_script", "GtaThread::Update(int)", &[Arg::Local0, Arg::TlsBase]),
    ("muguet_explosion", "CExplosionManager::AddExplosion(CExplosionManager::CExplosionArgs&, CProjectile*, bool)", &[Arg::Local0]),
    ("muguet_timewarp", "CTimeWarper::SetTargetTimeWarp(TW_Dir)", &[Arg::Local1Replaced]),
    ("muguet_text", "CTextFile::GetInternal(unsigned int)", &[Arg::MissingText]),
    ("muguet_txd", "graphics_commands::HasStreamedTxdLoaded(char const*)", &[Arg::TxdLoaded, Arg::TxdLoaded]),
    ("muguet_pool", "rage::sysMemGrowBuddyAllocator::GrowHeap()", &[Arg::PoolGrow]),
    ("muguet_cash", "stats_commands::CommandStatSetInt(int, int, bool)", &[Arg::CashWrite, Arg::CashWrite]),
    ("muguet_opus_frame", "opus_decode_frame", &[Arg::OpusFrame; 6]),
    ("muguet_resource_map", "rage::datResourceInfo::GenerateMap(rage::datResourceMap&) const", &[Arg::ResourceMap]),
];

const SCENARIO_CHECK: &str = "CScenarioManager::PerformCollisionChecksForScenarioPoint(CScenarioManager::CollisionCheckParams const&)";
const MOVIE_PRELOAD: &str = "CMovieMgr::PreloadMovie(char const*, bool)";

fn returns(args: &[Arg]) -> bool {
    args.contains(&Arg::Local1Replaced)
        || args.contains(&Arg::MissingText)
        || args.contains(&Arg::TxdLoaded)
        || args.contains(&Arg::PoolGrow)
        || args.contains(&Arg::CashWrite)
        || args.contains(&Arg::OpusFrame)
}

struct Patcher {
    imported_funcs: u32,
    types: u32,
    tls_global: u32,
    targets: HashMap<u32, usize>,
    scenario_check: Option<u32>,
    movie_preload: Option<u32>,
    pool_size: Option<u32>,
    exports: Vec<(String, u32)>,
    next_func: u32,
}

impl Patcher {
    fn shift(&self) -> u32 {
        HOOKS.len() as u32
    }
}

impl Reencode for Patcher {
    type Error = anyhow::Error;

    fn function_index(&mut self, f: u32) -> Result<u32, Error<Self::Error>> {
        Ok(if f >= self.imported_funcs { f + self.shift() } else { f })
    }

    fn parse_type_section(
        &mut self,
        types: &mut wasm_encoder::TypeSection,
        section: wasmparser::TypeSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        utils::parse_type_section(self, types, section)?;
        for (_, _, args) in HOOKS {
            types.ty().function(
                vec![I64; args.len()],
                if returns(args) { vec![I64] } else { vec![] },
            );
        }
        Ok(())
    }

    fn parse_import_section(
        &mut self,
        imports: &mut wasm_encoder::ImportSection,
        section: wasmparser::ImportSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        utils::parse_import_section(self, imports, section)?;
        for (k, (name, _, _)) in HOOKS.iter().enumerate() {
            imports.import(
                "env",
                name,
                wasm_encoder::EntityType::Function(self.types + k as u32),
            );
        }
        Ok(())
    }

    fn parse_export_section(
        &mut self,
        exports: &mut wasm_encoder::ExportSection,
        section: wasmparser::ExportSectionReader<'_>,
    ) -> Result<(), Error<Self::Error>> {
        utils::parse_export_section(self, exports, section)?;
        for (name, f) in std::mem::take(&mut self.exports) {
            exports.export(&name, wasm_encoder::ExportKind::Func, f + self.shift());
        }
        Ok(())
    }

    fn parse_function_body(
        &mut self,
        code: &mut wasm_encoder::CodeSection,
        func: wasmparser::FunctionBody<'_>,
    ) -> Result<(), Error<Self::Error>> {
        let index = self.next_func;
        self.next_func += 1;
        if Some(index) == self.scenario_check {
            use wasm_encoder::Instruction as I;
            let mut f = self.new_function_with_parsed_locals(&func)?;
            let mut reader = func.get_operators_reader()?;
            while !reader.eof() {
                let i = self.parse_instruction(&mut reader)?;
                f.instruction(if matches!(i, I::I32Const(6)) {
                    &I::I32Const(5)
                } else {
                    &i
                });
            }
            code.function(&f);
            return Ok(());
        }
        if Some(index) == self.movie_preload {
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
        let Some(&hook) = self.targets.get(&index) else {
            return utils::parse_function_body(self, code, func);
        };
        let mut f = self.new_function_with_parsed_locals(&func)?;
        use wasm_encoder::Instruction as I;
        let call = I::Call(self.imported_funcs + hook as u32);
        if HOOKS[hook].2 == [Arg::ResourceMap] {
            let mut reader = func.get_operators_reader()?;
            let mut body = Vec::new();
            while !reader.eof() {
                body.push(self.parse_instruction(&mut reader)?);
            }
            let end = body.pop();
            for i in &body {
                f.instruction(i);
            }
            f.instruction(&I::LocalGet(1)).instruction(&call);
            if let Some(e) = end {
                f.instruction(&e);
            }
            code.function(&f);
            return Ok(());
        }
        if HOOKS[hook].2 == [Arg::PoolGrow] {
            let Some(size) = self.pool_size else {
                return Err(Error::UserError(anyhow::anyhow!("GetHeapSize not found")));
            };
            f.instruction(&I::LocalGet(0))
                .instruction(&I::Call(size + self.shift()))
                .instruction(&call)
                .instruction(&I::I64Eqz);
            f.instruction(&I::If(wasm_encoder::BlockType::Empty));
            f.instruction(&I::I32Const(-1)).instruction(&I::Return);
            f.instruction(&I::End);
        } else if HOOKS[hook].2.contains(&Arg::OpusFrame) {
            for (k, wide) in [true, true, false, true, false, false].into_iter().enumerate() {
                f.instruction(&I::LocalGet(k as u32));
                if !wide {
                    f.instruction(&I::I64ExtendI32S);
                }
            }
            f.instruction(&call)
                .instruction(&I::I32WrapI64)
                .instruction(&I::Return);
        } else if HOOKS[hook].2 == [Arg::MissingText] {
            for (label, _) in MISSING_TEXT {
                f.instruction(&I::LocalGet(0))
                    .instruction(&I::I32Const(rp_proto::joaat(label) as i32))
                    .instruction(&I::I32Eq);
                f.instruction(&I::If(wasm_encoder::BlockType::Empty));
                f.instruction(&I::LocalGet(0))
                    .instruction(&I::I64ExtendI32U)
                    .instruction(&call)
                    .instruction(&I::Return);
                f.instruction(&I::End);
            }
        } else if HOOKS[hook].2.first() == Some(&Arg::CashWrite) {
            for k in 0..3 {
                f.instruction(&I::LocalGet(0))
                    .instruction(&I::I32Const(
                        rp_proto::joaat(&format!("SP{k}_TOTAL_CASH")) as i32,
                    ))
                    .instruction(&I::I32Eq);
                f.instruction(&I::If(wasm_encoder::BlockType::Empty));
                f.instruction(&I::LocalGet(1))
                    .instruction(&I::I64ExtendI32S)
                    .instruction(&I::GlobalGet(self.tls_global))
                    .instruction(&call)
                    .instruction(&I::I64Eqz)
                    .instruction(&I::I32Eqz);
                f.instruction(&I::If(wasm_encoder::BlockType::Empty))
                    .instruction(&I::I32Const(1))
                    .instruction(&I::Return)
                    .instruction(&I::End);
                f.instruction(&I::End);
            }
        } else if HOOKS[hook].2.first() == Some(&Arg::TxdLoaded) {
            f.instruction(&I::LocalGet(0))
                .instruction(&I::I64Const(0))
                .instruction(&call)
                .instruction(&I::Drop);
            let mut reader = func.get_operators_reader()?;
            let mut last = None;
            while !reader.eof() {
                let i = self.parse_instruction(&mut reader)?;
                if let (I::Return, Some(I::LocalGet(2))) = (&i, &last) {
                    f.instruction(&I::Drop);
                    f.instruction(&I::I64Const(0))
                        .instruction(&I::LocalGet(2))
                        .instruction(&I::I64ExtendI32U)
                        .instruction(&I::I64Const(1))
                        .instruction(&I::I64Add);
                    f.instruction(&call).instruction(&I::I32WrapI64);
                }
                f.instruction(&i);
                last = Some(i);
            }
            code.function(&f);
            return Ok(());
        } else {
            for arg in HOOKS[hook].2 {
                match arg {
                    Arg::Local0 => f.instruction(&I::LocalGet(0)),
                    Arg::TlsBase => f.instruction(&I::GlobalGet(self.tls_global)),
                    Arg::Local1Replaced => {
                        f.instruction(&I::LocalGet(1)).instruction(&I::I64ExtendI32U)
                    }
                    _ => unreachable!(),
                };
            }
            f.instruction(&call);
            if returns(HOOKS[hook].2) {
                f.instruction(&I::I32WrapI64);
                f.instruction(&I::LocalSet(1));
            }
        }
        let mut reader = func.get_operators_reader()?;
        while !reader.eof() {
            f.instruction(&self.parse_instruction(&mut reader)?);
        }
        code.function(&f);
        Ok(())
    }
}

pub fn is_patched(wasm: &[u8]) -> bool {
    Parser::new(0).parse_all(wasm).any(|p| match p {
        Ok(Payload::ImportSection(s)) => {
            s.into_imports().any(|i| i.is_ok_and(|i| i.name == HOOKS[0].0))
        }
        _ => false,
    })
}

pub fn patch_full(wasm: &[u8], wanted: &[(&str, &str)]) -> Result<Vec<u8>> {
    let (mut imported_funcs, mut types, mut tls_global) = (0u32, 0u32, None);
    let mut by_name: HashMap<String, u32> = HashMap::new();
    for payload in Parser::new(0).parse_all(wasm) {
        match payload? {
            Payload::TypeSection(s) => types = s.count(),
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
    if by_name.contains_key(HOOKS[0].0) {
        bail!("game.wasm is already patched");
    }
    let mut targets = HashMap::new();
    for (k, (_, func, _)) in HOOKS.iter().enumerate() {
        let f = *by_name
            .get(*func)
            .with_context(|| format!("{func} not found (no name section?)"))?;
        targets.insert(f, k);
    }
    let mut exports = Vec::new();
    if let Some(&malloc) = by_name.get("emscripten_builtin_malloc") {
        exports.push(("muguet:malloc".to_string(), malloc));
    }
    for (export, func) in wanted {
        match by_name.get(*func) {
            Some(&i) => exports.push((format!("mp:{export}"), i)),
            None => eprintln!("[rp-ios-aot] note: {func} not found; skipping export"),
        }
    }
    let mut p = Patcher {
        imported_funcs,
        types,
        tls_global: tls_global.unwrap_or(1),
        targets,
        scenario_check: by_name.get(SCENARIO_CHECK).copied(),
        movie_preload: by_name.get(MOVIE_PRELOAD).copied(),
        pool_size: by_name
            .get("rage::sysMemGrowBuddyAllocator::GetHeapSize() const")
            .copied(),
        exports,
        next_func: imported_funcs,
    };
    let mut module = wasm_encoder::Module::new();
    p.parse_core_module(&mut module, Parser::new(0), wasm)
        .map_err(|e| anyhow::anyhow!("patch game.wasm: {e:?}"))?;
    Ok(module.finish())
}

fn ios_engine_config(heap_gb: u64) -> Result<Config> {
    let mut config = Config::new();
    config.wasm_memory64(true);
    config.wasm_threads(true);
    config.shared_memory(true);
    config.wasm_bulk_memory(true);
    config.wasm_multi_memory(true);
    config.cranelift_opt_level(OptLevel::Speed);
    config.memory_reservation(heap_gb << 30);
    config.memory_guard_size(64 << 10);
    config.memory_may_move(false);
    unsafe {
        config.cranelift_flag_set("enable_heap_access_spectre_mitigation", "false");
        config.cranelift_flag_set("enable_table_access_spectre_mitigation", "false");
        config.cranelift_flag_enable("has_lse");
    }
    config
        .target("aarch64-apple-ios")
        .map_err(anyhow::Error::from)?;
    Ok(config)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage:\n  rp-ios-aot <input_game.wasm> <output_game.cwasm> [--heap-gb 5|8] [--patch-only]"
        );
        std::process::exit(1);
    }
    let input = &args[1];
    let output = &args[2];
    let patch_only = args.iter().any(|a| a == "--patch-only");
    let heap_gb = args
        .windows(2)
        .find(|w| w[0] == "--heap-gb")
        .and_then(|w| w[1].parse::<u64>().ok())
        .unwrap_or(5);

    let t0 = Instant::now();
    let wasm_bytes = std::fs::read(input).with_context(|| format!("read {input}"))?;
    println!(
        "[rp-ios-aot] Loaded {} ({:.2} MB)",
        input,
        wasm_bytes.len() as f64 / (1024.0 * 1024.0)
    );

    let patched = if is_patched(&wasm_bytes) {
        println!("[rp-ios-aot] Input is already patched with runtime hooks.");
        wasm_bytes
    } else {
        let tp = Instant::now();
        let p = patch_full(&wasm_bytes, COMMANDS)?;
        println!(
            "[rp-ios-aot] Patched 9 runtime/iOS-RAM hooks + {} MP/RP commands in {:?} ({:.2} MB)",
            COMMANDS.len(),
            tp.elapsed(),
            p.len() as f64 / (1024.0 * 1024.0)
        );
        p
    };

    if patch_only {
        std::fs::write(output, &patched)?;
        println!("[rp-ios-aot] Wrote patched Wasm -> {}", output);
        return Ok(());
    }

    println!(
        "[rp-ios-aot] Cross-compiling to aarch64-apple-ios native AOT (heap={} GB, parallel Cranelift)...",
        heap_gb
    );
    let tc = Instant::now();
    let config = ios_engine_config(heap_gb)?;
    let engine = Engine::new(&config).map_err(anyhow::Error::from)?;
    let cwasm = engine
        .precompile_module(&patched)
        .map_err(anyhow::Error::from)?;
    std::fs::write(output, &cwasm)?;
    println!(
        "[rp-ios-aot] Done in {:?} (total {:?})! Wrote {} ({:.2} MB)",
        tc.elapsed(),
        t0.elapsed(),
        output,
        cwasm.len() as f64 / (1024.0 * 1024.0)
    );
    Ok(())
}

