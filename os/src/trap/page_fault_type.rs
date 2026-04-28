#[derive(Debug, Clone, Copy)]
pub enum PageFaultType {
    Write,
    Read,
    Execute,
}
