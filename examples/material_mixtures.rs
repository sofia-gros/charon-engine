//! 密度5と3の純物質を質量4のcarrierへ混合し、途中保存から分離を再開します。
use charon_engine::{
    foundation::{
        EngineResult, StableKey,
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        id::WorldId,
        schema::Schema,
    },
    world::*,
};
fn key(text: &str) -> StableKey {
    StableKey::parse(text).expect("作例の固定キー")
}
fn p(x: i32) -> CellPoint {
    CellPoint { x, y: 0 }
}
fn config() -> MixtureEditConfig {
    MixtureEditConfig {
        schema: key("example:mixtures"),
        channel: key("example:composition"),
        journal: MaterialReceiptJournal {
            key: key("example:journal"),
            schema: key("example:receipts"),
        },
    }
}
fn reason(name: &str, kind: MaterialReceiptKind) -> MaterialChangeReason {
    MaterialChangeReason {
        key: key(name),
        version: 1,
        kind,
    }
}
fn core(world: &World, name: &str) -> EngineResult<CellCore> {
    Ok(CellCore {
        material: world.content().material_id(MaterialHandle(key(name)))?,
        ..CellCore::AIR
    })
}
fn change(
    point: CellPoint,
    expected: CellCore,
    output: CellCore,
    before: MixtureRef,
    after: MixtureRef,
) -> MixtureCellChange {
    MixtureCellChange {
        cell: MaterialCellChange {
            point,
            expected,
            output,
            payload: MaterialPayloadPolicy::KeepChannels,
        },
        expected: before,
        output: after,
    }
}
fn separate(world: &mut World, reference: MixtureRef) -> EngineResult<()> {
    let layer = world.content().layer_id(key("example:terrain"))?;
    let water = core(world, "example:water")?;
    let oil = core(world, "example:oil")?;
    let solution = core(world, "example:solution")?;
    world.edit(|edit| {
        edit.apply_mixture_changes(
            layer,
            config(),
            reason("example:separate", MaterialReceiptKind::Conversion),
            MixtureConservation::PreserveComponents,
            &[
                change(p(0), solution, water, reference, MixtureRef::NONE),
                change(p(1), solution, oil, reference, MixtureRef::NONE),
            ],
        )
    })?;
    Ok(())
}
fn main() -> EngineResult<()> {
    let mut registry = ContentRegistry::new(ContentLimits::default())?;
    registry.layer(LayerDef {
        key: key("example:terrain"),
        role: LayerRole::Terrain,
        render_order: 0,
        collision_group: 1,
        simulated: true,
    })?;
    for (name, mass) in [
        ("example:water", 5),
        ("example:oil", 3),
        ("example:solution", 4),
    ] {
        registry.register(MaterialBuilder::new(name, MaterialBase::Liquid)?.mass_per_cell(mass))?;
    }
    registry.schema(Schema::of::<MixtureRef>(key("example:ref"), 1, 32)?)?;
    registry.schema(Schema::of::<MixtureStore>(config().schema, 1, 8192)?)?;
    registry.schema(Schema::of::<MaterialReceiptLog>(
        config().journal.schema,
        1,
        16384,
    )?)?;
    registry.material_channel(
        key("example:terrain"),
        ChannelDefinition {
            key: config().channel,
            version: 1,
            value_schema: key("example:ref"),
            layout: GridLayout {
                origin: p(0),
                x_scale: GridScale::new(1, 1)?,
                y_scale: GridScale::new(1, 1)?,
            },
            default: MixtureRef::NONE,
            limits: GridLimits {
                chunks: 1,
                checkpoint_bytes: 100_000,
            },
        },
    )?;
    let content = registry.freeze()?;
    let mut world = World::new(
        WorldId::from_parts(0, 1)?,
        0,
        WorldBounds::new(p(0), 3, 1)?,
        content.clone(),
        WorldLimits::default(),
    )?;
    let store = MixtureStore::new(
        &world,
        MixtureLimits {
            entries: 4,
            components: 4,
            receipts: 8,
        },
    )?;
    let journal = MaterialReceiptLog::new(&world, config().journal.key, 8)?;
    let layer = world.content().layer_id(key("example:terrain"))?;
    let water = core(&world, "example:water")?;
    let oil = core(&world, "example:oil")?;
    let solution = core(&world, "example:solution")?;
    let composition = MixtureState::new(&[
        MixturePart {
            material: key("example:oil"),
            ratio: 2,
        },
        MixturePart {
            material: key("example:water"),
            ratio: 2,
        },
    ])?;
    let reference = world.edit(|edit| {
        edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
        edit.set_state(MixtureStore::key(), config().schema, &store)?;
        edit.set_state(config().journal.key, config().journal.schema, &journal)?;
        edit.apply_material_changes(
            layer,
            config().journal,
            reason("example:source", MaterialReceiptKind::Source),
            &[
                MaterialCellChange {
                    point: p(0),
                    expected: CellCore::AIR,
                    output: water,
                    payload: MaterialPayloadPolicy::ResetChannels,
                },
                MaterialCellChange {
                    point: p(1),
                    expected: CellCore::AIR,
                    output: oil,
                    payload: MaterialPayloadPolicy::ResetChannels,
                },
            ],
        )?;
        edit.register_mixture(config().schema, &composition)
    })?;
    world.edit(|edit| {
        edit.apply_mixture_changes(
            layer,
            config(),
            reason("example:mix", MaterialReceiptKind::Conversion),
            MixtureConservation::PreserveComponents,
            &[
                change(p(0), water, solution, MixtureRef::NONE, reference),
                change(p(1), oil, solution, MixtureRef::NONE, reference),
            ],
        )
    })?;
    let store: MixtureStore = world.state(MixtureStore::key())?;
    for amount in store.quantities(world.content(), solution.material, reference)? {
        assert_eq!(amount.tokens, MaterialQuantity::new(1, 2)?);
        println!(
            "混合cell一つ: {} token={}/{}、mass={}/{}",
            amount.material,
            amount.tokens.numerator(),
            amount.tokens.denominator(),
            amount.mass.numerator(),
            amount.mass.denominator()
        );
    }
    let snapshot = world.checkpoint(1_000_000)?;
    let mut resumed = World::restore(&snapshot, content, WorldLimits::default(), 1_000_000)?;
    separate(&mut world, reference)?;
    separate(&mut resumed, reference)?;
    assert_eq!(world.checkpoint(1_000_000)?, resumed.checkpoint(1_000_000)?);
    let store: MixtureStore = resumed.state(MixtureStore::key())?;
    store.validate_world(&resumed)?;
    assert_eq!(store.receipts().count(), 2);
    assert_eq!(resumed.channel_bytes(), 0);
    println!("発行mass8→混合mass8→分離mass8。混合・分離2件のwitnessと途中保存からの再開が一致");
    Ok(())
}
