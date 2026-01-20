//! PDF Object Identifier.

/// A PDF object identifier consisting of object number and generation number.
///
/// In PDF, objects are referenced by their object number and generation number.
/// The object number is unique for each object, and the generation number is
/// incremented when an object is replaced (relevant for incremental updates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId {
    /// The object number (must be > 0 for indirect objects).
    pub number: u32,
    /// The generation number (typically 0 for new documents).
    pub generation: u16,
}

impl ObjectId {
    /// Creates a new ObjectId with generation 0.
    #[inline]
    pub fn new(number: u32) -> Self {
        Self {
            number,
            generation: 0,
        }
    }

    /// Creates a new ObjectId with a specific generation number.
    #[inline]
    pub fn with_generation(number: u32, generation: u16) -> Self {
        Self { number, generation }
    }

    /// Returns the reference string for this object (e.g., "1 0 R").
    pub fn reference_string(&self) -> String {
        format!("{} {} R", self.number, self.generation)
    }

    /// Returns the object definition prefix (e.g., "1 0 obj").
    pub fn definition_string(&self) -> String {
        format!("{} {} obj", self.number, self.generation)
    }
}

impl From<(u32, u16)> for ObjectId {
    fn from((number, generation): (u32, u16)) -> Self {
        Self { number, generation }
    }
}

impl From<u32> for ObjectId {
    fn from(number: u32) -> Self {
        Self::new(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_id_new() {
        let id = ObjectId::new(1);
        assert_eq!(id.number, 1);
        assert_eq!(id.generation, 0);
    }

    #[test]
    fn test_object_id_with_generation() {
        let id = ObjectId::with_generation(5, 2);
        assert_eq!(id.number, 5);
        assert_eq!(id.generation, 2);
    }

    #[test]
    fn test_reference_string() {
        let id = ObjectId::new(1);
        assert_eq!(id.reference_string(), "1 0 R");

        let id2 = ObjectId::with_generation(10, 3);
        assert_eq!(id2.reference_string(), "10 3 R");
    }

    #[test]
    fn test_definition_string() {
        let id = ObjectId::new(1);
        assert_eq!(id.definition_string(), "1 0 obj");
    }

    #[test]
    fn test_from_tuple() {
        let id: ObjectId = (5, 2).into();
        assert_eq!(id.number, 5);
        assert_eq!(id.generation, 2);
    }

    #[test]
    fn test_from_u32() {
        let id: ObjectId = 7.into();
        assert_eq!(id.number, 7);
        assert_eq!(id.generation, 0);
    }
}
