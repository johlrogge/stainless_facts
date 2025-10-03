use serde::{Deserialize, Serialize};

/// Whether a fact asserts or retracts a value
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Assert,
    Retract,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_serializes_to_ron() {
        let op = Operation::Assert;
        let ron = ron::to_string(&op).unwrap();
        assert_eq!(ron, "Assert");
    }

    #[test]
    fn operation_deserializes_from_ron() {
        let ron = "Retract";
        let op: Operation = ron::from_str(ron).unwrap();
        assert_eq!(op, Operation::Retract);
    }

    #[test]
    fn operation_round_trips() {
        let original = Operation::Assert;
        let serialized = ron::to_string(&original).unwrap();
        let deserialized: Operation = ron::from_str(&serialized).unwrap();
        assert_eq!(original, deserialized);
    }
}
