use std::{net::SocketAddr, ops::RangeInclusive};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Status {
    NotConnected,
    Initializing(InitializationStep),
    NoRingCon,
    Active(u8),
    Disconnected,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum InitializationStep {
    Configuring = 0,
    McuState,
    McuConfiguration0,
    McuConfiguration1,
    Step4,
    Step5,
    Step6,
    Step7,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Configuration {
    pub udp_address: SocketAddr,
    pub osc_address: String,
    pub in_range: RangeInclusive<u8>,
    pub in_center: u8,
    pub out_range: RangeInclusive<f32>,
    pub out_idle: f32,
}
