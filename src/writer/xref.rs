//! PDF Cross-Reference Table handling.

use crate::types::ObjectId;

/// An entry in the cross-reference table.
#[derive(Debug, Clone, Copy)]
pub enum XrefEntry {
    /// A free entry (deleted or unused object).
    Free {
        /// Next free object number.
        next_free: u32,
        /// Generation number.
        generation: u16,
    },
    /// An in-use entry.
    InUse {
        /// Byte offset of the object in the file.
        offset: u64,
        /// Generation number.
        generation: u16,
    },
}

impl XrefEntry {
    /// Creates a new in-use entry.
    pub fn in_use(offset: u64, generation: u16) -> Self {
        XrefEntry::InUse { offset, generation }
    }

    /// Creates a new free entry.
    pub fn free(next_free: u32, generation: u16) -> Self {
        XrefEntry::Free {
            next_free,
            generation,
        }
    }

    /// Formats the entry for the xref table (20 bytes including newline).
    pub fn to_xref_line(&self) -> String {
        match self {
            XrefEntry::Free {
                next_free,
                generation,
            } => {
                format!("{:010} {:05} f \n", next_free, generation)
            }
            XrefEntry::InUse { offset, generation } => {
                format!("{:010} {:05} n \n", offset, generation)
            }
        }
    }
}

/// A cross-reference table for PDF objects.
#[derive(Debug, Default)]
pub struct XrefTable {
    entries: Vec<(u32, XrefEntry)>,
}

impl XrefTable {
    /// Creates a new empty xref table.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Adds an in-use object entry.
    pub fn add_object(&mut self, id: ObjectId, offset: u64) {
        self.entries
            .push((id.number, XrefEntry::in_use(offset, id.generation)));
    }

    /// Returns the number of entries (including object 0).
    pub fn size(&self) -> u32 {
        if self.entries.is_empty() {
            1 // Just the free entry for object 0
        } else {
            self.entries.iter().map(|(n, _)| *n).max().unwrap_or(0) + 1
        }
    }

    /// Generates the xref table content as a string.
    pub fn to_xref_string(&self) -> String {
        let mut result = String::new();
        let size = self.size();

        // Header
        result.push_str(&format!("xref\n0 {}\n", size));

        // Build a map for quick lookup
        let entry_map: std::collections::HashMap<u32, &XrefEntry> =
            self.entries.iter().map(|(n, e)| (*n, e)).collect();

        // Object 0 is always free and points to object 0 (end of free list)
        result.push_str(&XrefEntry::free(0, 65535).to_xref_line());

        // Write entries for objects 1 to size-1
        for obj_num in 1..size {
            if let Some(entry) = entry_map.get(&obj_num) {
                result.push_str(&entry.to_xref_line());
            } else {
                // Missing object - write as free
                result.push_str(&XrefEntry::free(0, 65535).to_xref_line());
            }
        }

        result
    }

    /// Returns an iterator over the entries.
    pub fn iter(&self) -> impl Iterator<Item = &(u32, XrefEntry)> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xref_entry_in_use() {
        let entry = XrefEntry::in_use(12345, 0);
        assert_eq!(entry.to_xref_line(), "0000012345 00000 n \n");
    }

    #[test]
    fn test_xref_entry_free() {
        let entry = XrefEntry::free(0, 65535);
        assert_eq!(entry.to_xref_line(), "0000000000 65535 f \n");
    }

    #[test]
    fn test_xref_table_size() {
        let mut table = XrefTable::new();
        assert_eq!(table.size(), 1); // Just object 0

        table.add_object(ObjectId::new(1), 100);
        assert_eq!(table.size(), 2); // Objects 0 and 1

        table.add_object(ObjectId::new(5), 500);
        assert_eq!(table.size(), 6); // Objects 0 through 5
    }

    #[test]
    fn test_xref_table_to_string() {
        let mut table = XrefTable::new();
        table.add_object(ObjectId::new(1), 15);
        table.add_object(ObjectId::new(2), 100);

        let output = table.to_xref_string();
        assert!(output.starts_with("xref\n0 3\n"));
        assert!(output.contains("0000000000 65535 f \n")); // Object 0
        assert!(output.contains("0000000015 00000 n \n")); // Object 1
        assert!(output.contains("0000000100 00000 n \n")); // Object 2
    }
}
