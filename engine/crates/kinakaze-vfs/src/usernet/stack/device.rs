use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use std::collections::VecDeque;

const MTU: usize = 1500;
const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct PacketDevice {
    pub input: Option<Vec<u8>>,
    output: VecDeque<Vec<u8>>,
    queued: usize,
}
impl PacketDevice {
    pub fn take(&mut self) -> Option<Vec<u8>> {
        let packet = self.output.pop_front()?;
        self.queued -= packet.len();
        Some(packet)
    }
    pub fn pending(&self) -> bool {
        !self.output.is_empty()
    }
}
pub(super) struct Rx(Vec<u8>);
pub(super) struct Tx<'a>(&'a mut PacketDevice);
impl Device for PacketDevice {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;
    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        if self.queued + MTU > OUTPUT_LIMIT {
            return None;
        }
        Some((Rx(self.input.take()?), Tx(self)))
    }
    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        (self.queued + MTU <= OUTPUT_LIMIT).then_some(Tx(self))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = MTU;
        caps.max_burst_size = Some(64);
        caps
    }
}
impl RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}
impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut bytes = vec![0; len];
        let result = f(&mut bytes);
        self.0.queued += bytes.len();
        self.0.output.push_back(bytes);
        result
    }
}
