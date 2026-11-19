/// Parsing outcome distinguishing complete frames from fragmented TCP reads.
#[derive(Debug, PartialEq, Eq)]
pub enum Status<T> {
    /// A complete frame was parsed (`(request, bytes_consumed)`).
    Complete(T, usize),
    /// More bytes are needed from the socket before parsing can finish.
    Incomplete,
}
