//! Clipboard-Abstraktion mit save/restore-Snapshot.
//!
//! Die Snapshot-Logik ist backend-unabhängig (unit-testbar); das echte
//! Backend ist NSPasteboard und erhält alle Inhaltstypen byte-genau.

use crate::error::{Result, TalkerError};

/// Ein Pasteboard-Item: Liste von (UTI-Typ, Rohdaten).
pub type ItemData = Vec<(String, Vec<u8>)>;

pub trait Pasteboard {
    /// Alle Items mit allen Typen lesen. Leeres Clipboard → leerer Vec.
    fn read_items(&self) -> Result<Vec<ItemData>>;
    /// Clipboard leeren und die Items schreiben. Leere Liste → Clipboard bleibt leer.
    fn write_items(&self, items: &[ItemData]) -> Result<()>;
    /// Clipboard leeren und reinen Text setzen.
    fn write_text(&self, text: &str) -> Result<()>;
}

/// Zustand des Clipboards vor der Injection.
#[derive(Debug, PartialEq)]
pub struct Snapshot {
    items: Vec<ItemData>,
}

pub fn save(pb: &dyn Pasteboard) -> Result<Snapshot> {
    Ok(Snapshot {
        items: pb.read_items()?,
    })
}

pub fn restore(pb: &dyn Pasteboard, snapshot: Snapshot) -> Result<()> {
    pb.write_items(&snapshot.items)
}

/// Echtes Backend: NSPasteboard.generalPasteboard.
pub struct NsPasteboard;

impl Pasteboard for NsPasteboard {
    fn read_items(&self) -> Result<Vec<ItemData>> {
        use objc2_app_kit::NSPasteboard;
        let pb = NSPasteboard::generalPasteboard();
        let Some(ns_items) = pb.pasteboardItems() else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        for item in ns_items.iter() {
            let mut entry: ItemData = Vec::new();
            for ty in item.types().iter() {
                if let Some(data) = item.dataForType(&ty) {
                    entry.push((ty.to_string(), data.to_vec()));
                }
            }
            items.push(entry);
        }
        Ok(items)
    }

    fn write_items(&self, items: &[ItemData]) -> Result<()> {
        use objc2::runtime::ProtocolObject;
        use objc2_app_kit::{NSPasteboard, NSPasteboardItem};
        use objc2_foundation::{NSArray, NSData, NSString};

        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        if items.is_empty() {
            return Ok(());
        }
        let ns_items: Vec<_> = items
            .iter()
            .map(|entry| {
                let item = NSPasteboardItem::new();
                for (ty, bytes) in entry {
                    let ty = NSString::from_str(ty);
                    let data = NSData::with_bytes(bytes);
                    item.setData_forType(&data, &ty);
                }
                ProtocolObject::from_retained(item)
            })
            .collect();
        let array = NSArray::from_retained_slice(&ns_items);
        if !pb.writeObjects(&array) {
            return Err(TalkerError::Clipboard(
                "NSPasteboard.writeObjects schlug fehl".into(),
            ));
        }
        Ok(())
    }

    fn write_text(&self, text: &str) -> Result<()> {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        use objc2_foundation::NSString;

        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ok = unsafe { pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) };
        if !ok {
            return Err(TalkerError::Clipboard(
                "NSPasteboard.setString schlug fehl".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// In-Memory-Fake mit denselben Semantiken wie NSPasteboard
    /// (write leert immer zuerst).
    struct FakePasteboard {
        items: RefCell<Vec<ItemData>>,
    }

    impl FakePasteboard {
        fn new() -> Self {
            Self {
                items: RefCell::new(Vec::new()),
            }
        }
    }

    impl Pasteboard for FakePasteboard {
        fn read_items(&self) -> Result<Vec<ItemData>> {
            Ok(self.items.borrow().clone())
        }
        fn write_items(&self, items: &[ItemData]) -> Result<()> {
            *self.items.borrow_mut() = items.to_vec();
            Ok(())
        }
        fn write_text(&self, text: &str) -> Result<()> {
            *self.items.borrow_mut() = vec![vec![(
                "public.utf8-plain-text".to_string(),
                text.as_bytes().to_vec(),
            )]];
            Ok(())
        }
    }

    fn text_item(text: &str) -> ItemData {
        vec![(
            "public.utf8-plain-text".to_string(),
            text.as_bytes().to_vec(),
        )]
    }

    #[test]
    fn save_restore_roundtrip_text() {
        let pb = FakePasteboard::new();
        pb.write_text("Nutzer-Inhalt").unwrap();

        let snapshot = save(&pb).unwrap();
        pb.write_text("talker test").unwrap();
        restore(&pb, snapshot).unwrap();

        assert_eq!(pb.read_items().unwrap(), vec![text_item("Nutzer-Inhalt")]);
    }

    #[test]
    fn save_restore_empty_clipboard_stays_empty() {
        let pb = FakePasteboard::new();

        let snapshot = save(&pb).unwrap();
        pb.write_text("talker test").unwrap();
        restore(&pb, snapshot).unwrap();

        assert!(pb.read_items().unwrap().is_empty());
    }

    #[test]
    fn save_restore_preserves_non_text_content_byte_identical() {
        let pb = FakePasteboard::new();
        let png: ItemData = vec![("public.png".to_string(), vec![0x89, 0x50, 0x4E, 0x47, 0x00])];
        pb.write_items(std::slice::from_ref(&png)).unwrap();

        let snapshot = save(&pb).unwrap();
        pb.write_text("talker test").unwrap();
        restore(&pb, snapshot).unwrap();

        assert_eq!(pb.read_items().unwrap(), vec![png]);
    }

    #[test]
    fn save_restore_preserves_multiple_items_and_types() {
        let pb = FakePasteboard::new();
        let mixed: Vec<ItemData> = vec![
            vec![
                ("public.utf8-plain-text".to_string(), b"hallo".to_vec()),
                ("public.html".to_string(), b"<b>hallo</b>".to_vec()),
            ],
            vec![("public.png".to_string(), vec![1, 2, 3])],
        ];
        pb.write_items(&mixed).unwrap();

        let snapshot = save(&pb).unwrap();
        pb.write_text("talker test").unwrap();
        restore(&pb, snapshot).unwrap();

        assert_eq!(pb.read_items().unwrap(), mixed);
    }
}
