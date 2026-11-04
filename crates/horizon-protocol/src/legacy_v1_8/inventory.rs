//! Minecraft Java 1.8.x (protocol 47) inventory packets.
//!
//! Version 47 uses numeric item IDs, damage values and *named* NBT roots.
//! Modern data-component item codecs MUST NOT be used for this wire format.
use crate::{Error, Result, buf::{Reader, Writer}};
use horizon_world::nbt::{self, NamedTag, Tag};

const MAX_SLOTS: usize = 2_048;
const MAX_ITEM_NBT: usize = 2_097_152;

fn nbt_limits() -> nbt::Limits {
    nbt::Limits { max_bytes: MAX_ITEM_NBT, max_nodes: 65_536, max_depth: 64 }
}

fn finished(reader: &Reader<'_>, kind: &str) -> Result<()> {
    if reader.remaining() == 0 { Ok(()) }
    else { Err(Error::new(format!("1.8 {kind}: {} trailing bytes", reader.remaining()))) }
}

/// Historical item-stack representation. The NBT root has a name (usually empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    pub numeric_id: i16,
    pub count: u8,
    pub damage: i16,
    pub tag: Option<NamedTag>,
}

/// Decode one 1.8 Slot, returning the number of bytes consumed. This can be
/// called repeatedly in WindowItems without guessing the size of an NBT tag.
pub fn decode_slot(data: &[u8]) -> Result<(Option<ItemStack>, usize)> {
    let mut r = Reader::new(data);
    let id = r.i16()?;
    if id == -1 { return Ok((None, r.position())); }
    if id < 0 { return Err(Error::new("invalid 1.8 item ID")); }
    let count = r.u8()?;
    let damage = r.i16()?;
    // 1.8 NBT: TAG_End (0) when absent, otherwise a named TAG_Compound.
    let start = r.position();
    let marker = *data.get(start).ok_or_else(|| Error::new("missing 1.8 item NBT marker"))?;
    let tag = if marker == 0 {
        r.u8()?;
        None
    } else {
        if marker != 10 { return Err(Error::new("1.8 item NBT root must be a compound")); }
        let (named, used) = nbt::read_named(&data[start..], nbt_limits())
            .map_err(|e| Error::new(format!("1.8 item NBT: {e}")))?;
        if !matches!(named.tag, Tag::Compound(_)) {
            return Err(Error::new("invalid 1.8 item NBT tag type"));
        }
        r.bytes(used)?;
        Some(named)
    };
    Ok((Some(ItemStack { numeric_id: id, count, damage, tag }), r.position()))
}

/// Write one 1.8 Slot; empty stacks are encoded solely as an i16 of -1.
pub fn write_slot(w: &mut Writer, item: Option<&ItemStack>) -> Result<()> {
    let Some(item) = item else { w.i16(-1); return Ok(()); };
    if item.numeric_id < 0 { return Err(Error::new("invalid 1.8 item ID")); }
    let tag_bytes = if let Some(ref tag) = item.tag {
        if !matches!(tag.tag, Tag::Compound(_)) {
            return Err(Error::new("1.8 stack requires a compound NBT root"));
        }
        Some(nbt::write_named(tag, nbt_limits())
            .map_err(|e| Error::new(format!("1.8 item NBT: {e}")))?)
    } else { None };
    w.i16(item.numeric_id);
    w.u8(item.count);
    w.i16(item.damage);
    match tag_bytes {
        Some(bytes) => w.bytes(&bytes),
        None => w.u8(0),
    }
    Ok(())
}

/// Clientbound SetSlot (0x2f); window -1 and -2 have special inventory meanings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetSlot {
    pub window_id: i8,
    pub slot: i16,
    pub item: Option<ItemStack>,
}
pub fn decode_set_slot(data: &[u8]) -> Result<SetSlot> {
    let mut r = Reader::new(data);
    let window_id = r.i8()?;
    let slot = r.i16()?;
    let (item, consumed) = decode_slot(&data[r.position()..])?;
    r.bytes(consumed)?;
    finished(&r, "SetSlot")?;
    Ok(SetSlot { window_id, slot, item })
}

/// Clientbound WindowItems (0x30). A complete server-authoritative sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowItems {
    pub window_id: u8,
    pub items: Vec<Option<ItemStack>>,
}
pub fn decode_window_items(data: &[u8]) -> Result<WindowItems> {
    let mut r = Reader::new(data);
    let window_id = r.u8()?;
    let n = r.i16()?;
    if n < 0 || usize::try_from(n).unwrap_or(usize::MAX) > MAX_SLOTS {
        return Err(Error::new("invalid 1.8 inventory slot count"));
    }
    let mut items = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let (stack, consumed) = decode_slot(&data[r.position()..])?;
        r.bytes(consumed)?;
        items.push(stack);
    }
    finished(&r, "WindowItems")?;
    Ok(WindowItems { window_id, items })
}

/// Clientbound ConfirmTransaction (0x32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmTransaction {
    pub window_id: u8,
    pub action_number: i16,
    pub accepted: bool,
}
pub fn decode_confirm_transaction(data: &[u8]) -> Result<ConfirmTransaction> {
    let mut r = Reader::new(data);
    let out = ConfirmTransaction { window_id: r.u8()?, action_number: r.i16()?, accepted: r.bool()? };
    finished(&r, "ConfirmTransaction")?;
    Ok(out)
}

/// Serverbound 0x0f. Must acknowledge a rejected inventory transaction before
/// sending further clicks. Inventory state must follow server corrections.
pub fn confirm_transaction_body(ack: ConfirmTransaction) -> Vec<u8> {
    let mut w = Writer::new();
    w.varint(0x0f); w.u8(ack.window_id); w.i16(ack.action_number); w.bool(ack.accepted);
    w.data
}

/// Client click action (C0E, 0x0e): pickup, shift-click, hotbar swap,
/// creative middle-click, drop, drag and double-click modes (0..6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Click {
    pub window_id: u8,
    pub slot: i16,
    pub button: u8,
    pub action_number: i16,
    pub mode: u8,
}
pub fn click_window_body(click: Click, clicked_item: Option<&ItemStack>) -> Result<Vec<u8>> {
    if click.mode > 6 { return Err(Error::new("invalid 1.8 inventory click mode")); }
    let mut w = Writer::new();
    w.varint(0x0e);
    w.u8(click.window_id); w.i16(click.slot); w.u8(click.button);
    w.i16(click.action_number); w.u8(click.mode);
    write_slot(&mut w, clicked_item)?;
    Ok(w.data)
}

/// Serverbound 0x09: select one of the nine hotbar slots.
pub fn held_item_change_body(slot: u8) -> Result<Vec<u8>> {
    if slot >= 9 { return Err(Error::new("1.8 hotbar slot is outside 0..8")); }
    let mut w = Writer::new();
    w.varint(0x09); w.i16(i16::from(slot));
    Ok(w.data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_world::nbt::JavaString;

    #[test]
    fn slot_round_trips_named_nbt_and_empty() {
        let tag = NamedTag {
            name: JavaString::from(""),
            tag: Tag::compound([("display", Tag::String(JavaString::from("Sword")))]),
        };
        let item = ItemStack { numeric_id: 276, count: 1, damage: 4, tag: Some(tag) };
        let mut w = Writer::new();
        write_slot(&mut w, Some(&item)).unwrap();
        let (decoded, used) = decode_slot(&w.data).unwrap();
        assert_eq!(used, w.data.len());
        assert_eq!(decoded, Some(item));
        let mut empty = Writer::new();
        write_slot(&mut empty, None).unwrap();
        assert_eq!(empty.data, [255, 255]);
        assert_eq!(decode_slot(&empty.data).unwrap(), (None, 2));
    }

    #[test]
    fn window_sync_and_single_slot_packets() {
        let mut w = Writer::new();
        w.u8(0); w.i16(3);
        write_slot(&mut w, None).unwrap();
        write_slot(&mut w, Some(&ItemStack { numeric_id: 1, count: 64, damage: 0, tag: None })).unwrap();
        write_slot(&mut w, None).unwrap();
        let window = decode_window_items(&w.data).unwrap();
        assert_eq!(window.items.len(), 3);
        assert_eq!(window.items[1].as_ref().unwrap().count, 64);
        let mut set = Writer::new();
        set.i8(-1); set.i16(-1); write_slot(&mut set, None).unwrap();
        assert_eq!(decode_set_slot(&set.data).unwrap().slot, -1);
        set.u8(9);
        assert!(decode_set_slot(&set.data).is_err());
    }

    #[test]
    fn click_and_transaction_wire() {
        let click = Click { window_id: 1, slot: 5, button: 0, action_number: 2, mode: 0 };
        assert_eq!(click_window_body(click, None).unwrap(), [14,1,0,5,0,0,2,0,255,255]);
        let ack = ConfirmTransaction { window_id: 1, action_number: 2, accepted: false };
        assert_eq!(confirm_transaction_body(ack), [15,1,0,2,0]);
        assert_eq!(decode_confirm_transaction(&[1,0,2,0]).unwrap(), ack);
        assert!(held_item_change_body(9).is_err());
    }

    #[test]
    fn malformed_slot_count_and_nbt_rejected() {
        assert!(decode_window_items(&[1, 255,255]).is_err());
        assert!(decode_window_items(&[1, 0,1, 0]).is_err());
        assert!(decode_slot(&[255,254]).is_err());
        assert!(decode_slot(&[0,1,1,0,0,10]).is_err());
    }
}
