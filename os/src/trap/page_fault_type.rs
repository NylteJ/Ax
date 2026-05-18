#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultType {
    Write,
    Read,
    Execute,
}
