pub trait ChatProvider {
    fn ask(&self, question: &str, image: &[u8]) -> Result<String, ProviderError>;
}

#[derive(Debug, PartialEq)]
pub struct ProviderError(pub String);

pub struct StubProvider;

impl ChatProvider for StubProvider {
    fn ask(&self, question: &str, _image: &[u8]) -> Result<String, ProviderError> {
        Ok(format!("stub answer: {question}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_returns_canned_answer() {
        let provider = StubProvider;
        let answer = provider.ask("what is on my screen?", &[1, 2, 3]).unwrap();
        assert_eq!(answer, "stub answer: what is on my screen?");
    }
}
