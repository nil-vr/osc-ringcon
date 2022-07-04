use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    ops::RangeInclusive,
    time::{Duration, Instant},
};

use crossbeam_channel::RecvTimeoutError;
use ipc_channel::ipc::{IpcReceiver, IpcSender};
use joycon_rs::{
    joycon::{
        joycon_features::JoyConFeature,
        lights::{Flash, LightUp, Lights},
    },
    prelude::*,
};

use crate::messages::{Configuration, InitializationStep, Status};

trait AsSubCommandRaw: Copy {
    fn as_sub_command_raw(self) -> u8;
}

impl AsSubCommandRaw for SubCommand {
    fn as_sub_command_raw(self) -> u8 {
        self as u8
    }
}

impl AsSubCommandRaw for u8 {
    fn as_sub_command_raw(self) -> u8 {
        self
    }
}

fn repeat_sub_command<S: AsSubCommandRaw, F: FnMut(&[u8; 362]) -> Option<V>, V>(
    driver: &mut SimpleJoyConDriver,
    sub_command: S,
    data: &[u8],
    mut cb: F,
) -> Result<V, JoyConError> {
    loop {
        let data = match driver.send_sub_command_raw(sub_command.as_sub_command_raw(), data) {
            Ok(data) => data,
            Err(JoyConError::SubCommandError(_, _)) => continue,
            Err(error) => return Err(error),
        };
        if let SubCommandReply::Checked(data) = data {
            if let Some(value) = cb(&data) {
                return Ok(value);
            }
        } else {
            unreachable!();
        }
    }
}

struct OscOut {
    socket: UdpSocket,
    target: SocketAddr,
    buffer: Vec<u8>,
    mid_in: u8,
    mid_out: f32,
    factor_low: f32,
    factor_high: f32,
    range_out: RangeInclusive<f32>,
    idle_out: f32,
}

impl OscOut {
    pub fn new() -> Self {
        Self {
            socket: UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .unwrap(),
            target: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
            buffer: Vec::new(),
            mid_in: 0,
            mid_out: 0.75,
            factor_low: 0.0,
            factor_high: 0.0,
            range_out: 0.5..=1.0,
            idle_out: 0.0,
        }
    }

    pub fn configure(&mut self, config: &Configuration) {
        self.target = config.udp_address;

        self.buffer.clear();
        // null terminated address string, padded to 4 byte boundaries,
        // followed by type code and float.
        self.buffer
            .reserve(((config.osc_address.len() + 4) & !3) + 8);
        self.buffer.extend_from_slice(config.osc_address.as_bytes());
        self.buffer.push(0);
        let align = ((self.buffer.len() - 1 + 4) & !3) - self.buffer.len();
        self.buffer.extend_from_slice(&[0, 0, 0][..align]);
        self.buffer.extend_from_slice(b",f\0\0\0\0\0\0");

        self.mid_in = config.in_center;
        let half_out = (config.out_range.end() - config.out_range.start()) / 2.0;
        self.mid_out = config.out_range.start() + half_out;
        self.factor_low = half_out / (config.in_range.end() - config.in_center) as f32;
        self.factor_high = -half_out / (config.in_center - config.in_range.start()) as f32;
        self.range_out = f32::min(*config.out_range.start(), *config.out_range.end())..=f32::max(*config.out_range.start(), *config.out_range.end());

        self.idle_out = config.out_idle;
    }

    pub fn send(&mut self, flex: u8) {
        if self.buffer.is_empty() {
            return;
        }

        let fflex = if flex == 0 {
            self.idle_out
        } else if flex == self.mid_in {
            self.mid_out
        } else if flex < self.mid_in {
            (self.mid_out + (self.mid_in - flex) as f32 * self.factor_low).clamp(*self.range_out.start(), *self.range_out.end())
        } else {
            (self.mid_out + (flex - self.mid_in) as f32 * self.factor_high).clamp(*self.range_out.start(), *self.range_out.end())
        };

        let range = self.buffer.len() - 4..;
        self.buffer[range].copy_from_slice(&fflex.to_be_bytes());
        self.socket.send_to(&self.buffer, self.target).unwrap();

        println!("Flex: {}", fflex);
    }
}

pub(crate) fn joycon_main(
    config: IpcReceiver<Configuration>,
    status: IpcSender<Status>,
) -> Result<(), JoyConError> {
    let mut osc_out = OscOut::new();
    let manager = JoyConManager::get_instance();
    let devices = {
        let lock = manager.lock().unwrap();
        lock.new_devices()
    };

    status.send(Status::NotConnected).unwrap();

    // Wait for a right joycon
    loop {
        let device = match devices.recv_timeout(Duration::from_secs(1)) {
            Ok(device) => device,
            Err(RecvTimeoutError::Timeout) => {
                while let Ok(config) = config.try_recv() {
                    osc_out.configure(&config);
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => panic!("Unexpected joycon-rs error"),
        };

        {
            let device = device.lock().unwrap();
            if device.device_type() != JoyConDeviceType::JoyConR {
                continue;
            }
        }

        let mut driver = SimpleJoyConDriver::new(&device)?;

        // This initialization sequence is based on ringrunnermg/Ringcon-Driver:
        // https://github.com/ringrunnermg/Ringcon-Driver/blob/76cad33bd545d5511eee31ef238d6a30f42e72d6/Ringcon%20Driver/joycon.hpp

        println!("step 0");
        status
            .send(Status::Initializing(InitializationStep::Configuring))
            .unwrap();

        driver.joycon().set_blocking_mode(true)?;
        driver.enable_feature(JoyConFeature::Vibration)?;
        driver.send_sub_command(SubCommand::EnableIMU, &[0x01])?;
        driver.send_sub_command(SubCommand::SetInputReportMode, &[0x30])?;

        // step 1
        println!("step 1");
        status
            .send(Status::Initializing(InitializationStep::McuConfiguration0))
            .unwrap();
        repeat_sub_command(
            &mut driver,
            SubCommand::Set_NFC_IR_MCUState,
            &[0x01],
            |data| {
                if data[0xd] == 0x80 && data[0xe] == 0x22 {
                    Some(())
                } else {
                    None
                }
            },
        )?;

        // no step 2

        // step 3
        println!("step 2");
        status
            .send(Status::Initializing(InitializationStep::McuConfiguration1))
            .unwrap();
        repeat_sub_command(
            &mut driver,
            SubCommand::Set_NFC_IR_MCUConfiguration,
            &[
                0x21, 0x00, 0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xfa,
            ],
            |data| {
                if data[0] == 0x21 && data[15] == 1 && data[22] == 3 {
                    Some(())
                } else {
                    None
                }
            },
        )?;

        // no step 4

        // step 5
        println!("step 3");
        status
            .send(Status::Initializing(InitializationStep::McuState))
            .unwrap();
        repeat_sub_command(
            &mut driver,
            SubCommand::Set_NFC_IR_MCUConfiguration,
            &[
                0x21, 0x01, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xf3,
            ],
            |data| {
                if data[0] == 0x21 && data[15] == 9 && data[17] == 1 {
                    Some(())
                } else {
                    None
                }
            },
        )?;

        // step 6
        println!("step 4");
        status
            .send(Status::Initializing(InitializationStep::Step4))
            .unwrap();
        repeat_sub_command(&mut driver, 0x59, &[], |data| {
            if data[0] == 0x21 && data[14] == 0x59 && data[16] == 0x20 {
                Some(())
            } else {
                None
            }
        })?;

        // step 7
        println!("step 5");
        status
            .send(Status::Initializing(InitializationStep::Step5))
            .unwrap();
        driver.send_sub_command(SubCommand::EnableIMU, &[0x03])?;
        driver.send_sub_command(SubCommand::EnableIMU, &[0x02])?;
        driver.send_sub_command(SubCommand::EnableIMU, &[0x01])?;

        repeat_sub_command(
            &mut driver,
            0x5c,
            &[
                0x06, 0x03, 0x25, 0x06, 0x00, 0x00, 0x00, 0x00, 0x1c, 0x16, 0xed, 0x34, 0x36, 0x00,
                0x00, 0x00, 0x0a, 0x64, 0x0b, 0xe6, 0xa9, 0x22, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x90, 0xa8, 0xe1, 0x34, 0x36,
            ],
            |data| {
                if data[0] == 0x21 && data[14] == 0x5c {
                    Some(())
                } else {
                    None
                }
            },
        )?;

        // step 8
        println!("step 6");
        status
            .send(Status::Initializing(InitializationStep::Step6))
            .unwrap();
        repeat_sub_command(&mut driver, 0x5a, &[0x04, 0x01, 0x01, 0x02], |data| {
            if data[0] == 0x21 && data[14] == 0x5a {
                Some(())
            } else {
                None
            }
        })?;

        // step 13
        println!("step 7");
        status
            .send(Status::Initializing(InitializationStep::Step7))
            .unwrap();
        repeat_sub_command(&mut driver, 0x58, &[0x04, 0x04, 0x12, 0x02], |data| {
            if data[0] == 0x21 && data[14] == 0x58 {
                Some(())
            } else {
                None
            }
        })?;

        println!("initialized");
        driver.set_player_lights(&[LightUp::LED0], &[Flash::LED0])?;

        let mut last_update: Option<(u8, Instant)> = None;
        const MAX_INTERVAL: Duration = Duration::from_secs(1);
        loop {
            let mut buf = [0u8; 362];
            let len = match driver.read(&mut buf) {
                Ok(len) => len,
                Err(error) => {
                    // Send a zero to indicate the controller is gone.
                    osc_out.send(0);
                    status.send(Status::Disconnected).unwrap();
                    eprintln!("{:?}", error);
                    return Err(error);
                }
            };
            let data = &buf[..len];

            if data[0] != 0x30 || data.len() < 40 {
                continue;
            }

            let flex = data[40];
            let now = Instant::now();
            if let Some(last_update) = last_update {
                if last_update.0 == flex && now.duration_since(last_update.1) < MAX_INTERVAL {
                    continue;
                }
            }
            last_update = Some((flex, now));

            if let Ok(conf) = config.try_recv() {
                osc_out.configure(&conf);
            }

            osc_out.send(flex);

            if flex == 0 {
                status.send(Status::NoRingCon).unwrap();
            } else {
                status.send(Status::Active(flex)).unwrap();
            }
        }
    }
}
