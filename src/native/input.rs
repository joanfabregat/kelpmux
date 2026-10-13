use super::protocol::*;
#[derive(Default)]
pub struct Decoder;
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<InputEvent> {
        vec![InputEvent::Bytes(bytes.to_vec())]
    }
    pub fn flush(&mut self) -> Vec<InputEvent> {
        vec![]
    }
}
#[derive(Default)]
pub struct Router;
impl Router {
    pub fn route(&mut self, _event: InputEvent, _frame: &Frame) -> Vec<ClientMessage> {
        vec![]
    }
}
