#![windows_subsystem = "windows"]

use fluent_bundle::FluentArgs;
use font_kit::source::SystemSource;
use futures::channel::mpsc;
use iced::{
    executor, Application, Column, Command, Container, Element, Length, ProgressBar, Settings,
    Subscription, Text,
};
use iced_native::subscription;
use internationalization::Resources;
use messages::{Configuration, Status};
use std::any::TypeId;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;

mod agent;
mod internationalization;
mod joycon;
mod messages;

struct App {
    resources: Resources,
    status: Status,
    current_config: Configuration,
    _config_tx: mpsc::Sender<Configuration>,
    status_rx: watch::Receiver<Status>,
}

#[derive(Debug)]
enum Message {
    Status(Status),
}

impl Application for App {
    type Executor = executor::Default;
    type Message = Message;
    type Flags = Option<Resources>;

    fn new(resources: Option<Resources>) -> (App, Command<Message>) {
        let (mut config_tx, status_rx) = agent::spawn();
        let config = Configuration {
            udp_address: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9000)),
            osc_address: "/avatar/parameters/ringcon_flex".to_string(),
            in_center: 15,
            in_range: 7..=24,
            out_idle: 0.0,
            out_range: 0.5..=1.0,
        };
        config_tx.try_send(config.clone()).unwrap();

        (
            App {
                status: Status::NotConnected,
                current_config: config,
                _config_tx: config_tx,
                status_rx,
                resources: resources.unwrap(),
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        self.resources.get_string("title").into_owned()
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Status(status) => {
                self.status = status;
            }
        }
        Command::none()
    }

    fn view(&mut self) -> Element<Message> {
        let mut column = Column::new().spacing(20);

        match &self.status {
            Status::NotConnected => {
                column = column.push(Text::new(
                    self.resources.get_string("connect-joycon").into_owned(),
                ));
            }
            Status::Initializing(step) => {
                column = column
                    .push(Text::new(
                        self.resources
                            .get_string("initializing-joycon")
                            .into_owned(),
                    ))
                    .push(ProgressBar::new(0.0..=1.0, *step as i32 as f32 / 8.0));
            }
            Status::NoRingCon => {
                column = column.push(Text::new(
                    self.resources.get_string("connect-ringcon").into_owned(),
                ));
            }
            Status::Active(flex) => {
                let mut args = FluentArgs::new();
                args.set("min", *self.current_config.in_range.start());
                args.set("flex", *flex);
                args.set("max", *self.current_config.in_range.end());
                let mut errors = Vec::new();
                let text = self
                    .resources
                    .bundles()
                    .format_value_sync("status-flex", Some(&args), &mut errors)
                    .unwrap()
                    .map(|c| c.into_owned())
                    .unwrap_or_default();
                column = column.push(Text::new(text)).push(ProgressBar::new(
                    (*self.current_config.in_range.start() as f32)
                        ..=(*self.current_config.in_range.end() as f32),
                    *flex as f32,
                ));
            }
            Status::Disconnected => {
                column = column.push(Text::new(
                    self.resources.get_string("restarting").into_owned(),
                ));
            }
        }

        Container::new(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y()
            .padding(20)
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        subscription::run(
            TypeId::of::<Status>(),
            WatchStream::new(self.status_rx.clone()),
        )
        .map(Message::Status)
    }
}

fn load_font<I: IntoIterator<Item = V>, V: AsRef<str>>(names: I) -> Option<&'static [u8]> {
    let font_source = SystemSource::new();

    fn try_load_font(source: &SystemSource, name: &str) -> Option<&'static [u8]> {
        let family = source.select_family_by_name(name).ok()?;
        let font = family.fonts().iter().next()?.load().ok()?;
        let data = font.copy_font_data()?;
        Some((*data).clone().leak())
    }

    names
        .into_iter()
        .filter_map(|name| try_load_font(&font_source, name.as_ref()))
        .next()
}

fn main() -> anyhow::Result<()> {
    if std::env::args().skip(1).collect::<Vec<_>>() == ["agent"] {
        return agent::run();
    }

    let resources = internationalization::Resources::new();

    let font = load_font(resources.fonts());

    App::run(Settings {
        default_font: font,
        flags: Some(resources),
        window: iced::window::Settings {
            size: (384, 128),
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();

    Ok(())
}
