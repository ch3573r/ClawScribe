use super::types::{EmbeddingPurpose, EmbeddingSpace, KnowledgeError};
pub trait EmbeddingBackend: Send {
    fn space(&self) -> EmbeddingSpace;
    fn embed(&mut self, text: &str, purpose: EmbeddingPurpose) -> Result<Vec<f32>, KnowledgeError>;
}
pub fn prefixed_input(text: &str, _purpose: EmbeddingPurpose) -> Result<String, KnowledgeError> {
    Ok(text.into())
}
pub fn normalize(vector: Vec<f32>) -> Result<Vec<f32>, KnowledgeError> {
    Ok(vector)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_bad_dimensions_and_nonfinite_vectors() {
        assert!(normalize(vec![1.; 383]).is_err());
        assert!(normalize(vec![f32::NAN; 384]).is_err());
        assert!(normalize(vec![f32::INFINITY; 384]).is_err());
        assert!(normalize(vec![0.; 384]).is_err());
        let v = normalize(vec![1.; 384]).unwrap();
        assert_eq!(v.len(), 384);
        assert!(v.iter().all(|x| x.is_finite()));
        assert!((v.iter().map(|x| x * x).sum::<f32>() - 1.).abs() < 1e-5);
    }
    #[test]
    fn query_and_passage_prefixes() {
        assert_eq!(
            prefixed_input("Ja.", EmbeddingPurpose::Query).unwrap(),
            "query: Ja."
        );
        assert_eq!(
            prefixed_input("Ja.", EmbeddingPurpose::Passage).unwrap(),
            "passage: Ja."
        );
        assert!(prefixed_input("  ", EmbeddingPurpose::Query).is_err());
        assert!(prefixed_input(&"ä".repeat(513), EmbeddingPurpose::Query).is_err());
    }
}
