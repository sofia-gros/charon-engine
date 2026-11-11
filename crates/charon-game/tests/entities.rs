use charon_game::{EntityError, EntityId, EntityWorld};

#[test]
fn entity_ids_are_monotonic() {
    let mut entities = EntityWorld::new();
    let first = entities.spawn(-16, 0, 8, 16).unwrap();
    let second = entities.spawn(0, 0, 8, 16).unwrap();
    assert_eq!(first, EntityId(1));
    assert_eq!(second, EntityId(2));
    assert_eq!(entities.remove(first).unwrap().id, first);
    let third = entities.spawn(16, 0, 8, 16).unwrap();
    assert_eq!(third, EntityId(3));
    assert_eq!(entities.iter().map(|entity| entity.id).collect::<Vec<_>>(), [second, third]);
    assert_eq!(entities.get(first), None);
    assert_eq!(entities.spawn(0, 0, 0, 1), Err(EntityError::InvalidDimensions));
    assert_eq!(entities.spawn(0, 0, 33, 1), Err(EntityError::InvalidDimensions));
    assert_eq!(
        entities.spawn((i64::from(i32::MAX) + 1) * 16, 0, 1, 1),
        Err(EntityError::OutOfBounds)
    );
}

#[test]
fn entity_hash_is_stable() {
    let mut a = EntityWorld::new();
    let mut b = EntityWorld::new();
    assert_eq!(a.state_hash(), b.state_hash());
    for entities in [&mut a, &mut b] {
        let id = entities.spawn(-17, 32, 8, 16).unwrap();
        entities.spawn(0, 0, 1, 1).unwrap();
        entities.remove(id);
    }
    assert_eq!(a.state_hash(), b.state_hash());
    let before = a.state_hash();
    a.spawn(16, 0, 8, 16).unwrap();
    a.remove(EntityId(3));
    assert_ne!(a.state_hash(), before, "next ID is part of the hash");
    let id = b.spawn(16, 0, 8, 16).unwrap();
    b.remove(id);
    assert_eq!(a.state_hash(), b.state_hash());

    // 位置の違いもハッシュに反映する。両方の next ID は同じ。
    let mut c = EntityWorld::new();
    let mut d = EntityWorld::new();
    c.spawn(0, 0, 8, 16).unwrap();
    d.spawn(16, 0, 8, 16).unwrap();
    assert_ne!(c.state_hash(), d.state_hash());
}
