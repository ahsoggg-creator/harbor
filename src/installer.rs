//! Installer / uninstaller window. The same executable becomes the installer when it is named `*Setup*.exe`
//! (or started with `--install`) and the uninstaller with `--uninstall`.

use crate::{theme, winint};
use eframe::egui::{self, Align, Layout, RichText, Ui};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Install,
    Uninstall,
}

pub struct SetupApp {
    kind: Kind,
    desktop: bool,
    delete_settings: bool,
    job: Option<Receiver<Result<Option<PathBuf>, String>>>,
    result: Option<Result<Option<PathBuf>, String>>,
}

impl SetupApp {
    pub fn new(cc: &eframe::CreationContext<'_>, kind: Kind) -> Self {
        theme::install(&cc.egui_ctx);
        Self {
            kind,
            desktop: true,
            delete_settings: false,
            job: None,
            result: None,
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let (kind, desktop, wipe, ctx) = (self.kind, self.desktop, self.delete_settings, ctx.clone());
        std::thread::spawn(move || {
            let r = match kind {
                Kind::Install => winint::install(desktop).map(Some),
                Kind::Uninstall => winint::uninstall(wipe).map(|()| None),
            };
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.job = Some(rx);
    }

    fn body(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok(r) => {
                    self.result = Some(r);
                    self.job = None;
                }
                Err(_) => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(24.0);
                        ui.add(egui::Spinner::new().size(32.0).color(theme::ACCENT));
                        ui.label("Подождите…");
                    });
                    ctx.request_repaint();
                    return;
                }
            }
        }
        let close = |ui: &Ui| ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        match (&self.result, self.kind) {
            (Some(Ok(exe)), kind) => {
                let exe = exe.clone();
                ui.vertical_centered(|ui| {
                    let done = if kind == Kind::Install {
                        "Готово: Harbor установлен"
                    } else {
                        "Harbor удалён"
                    };
                    ui.label(RichText::new(done).size(18.0).color(theme::GOOD).strong());
                    if kind == Kind::Install {
                        ui.label("Ярлык есть в меню «Пуск». Скачанные программы лежат в «Загрузки\\Harbor».");
                    } else {
                        ui.label("Скачанные программы в папке загрузок не тронуты.");
                    }
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 200.0).max(0.0) / 2.0);
                        if let Some(exe) = &exe
                            && theme::primary_button(ui, "Запустить").clicked()
                        {
                            let _ = std::process::Command::new(exe).spawn();
                            close(ui);
                        }
                        if ui.button("Закрыть").clicked() {
                            close(ui);
                        }
                    });
                });
            }
            (Some(Err(e)), _) => {
                ui.label(RichText::new("Не получилось").strong().color(theme::BAD));
                ui.label(e.as_str());
                if ui.button("Повторить").clicked() {
                    self.result = None;
                }
            }
            (None, Kind::Install) => {
                let dir = winint::install_dir()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                let upgrade = winint::installed_exe().is_some_and(|e| e.exists());
                theme::card(false).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Папка установки").color(theme::SUBTEXT));
                    ui.label(dir);
                    ui.add_space(6.0);
                    ui.checkbox(&mut self.desktop, "Ярлык на рабочем столе");
                    ui.label(
                        RichText::new("Права администратора не нужны: установка только для вашей учётной записи.")
                            .small()
                            .color(theme::SUBTEXT),
                    );
                });
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Отмена").clicked() {
                        close(ui);
                    }
                    if theme::primary_button(
                        ui,
                        if upgrade {
                            "Обновить"
                        } else {
                            "Установить"
                        },
                    )
                    .clicked()
                    {
                        self.start(&ctx);
                    }
                });
            }
            (None, Kind::Uninstall) => {
                ui.vertical_centered(|ui| {
                    ui.label("Удалить Harbor с этого компьютера?");
                    ui.label(
                        RichText::new("Программы, скачанные через Harbor, останутся на диске.").color(theme::SUBTEXT),
                    );
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.delete_settings, "Удалить также настройки и кэш списка");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 180.0).max(0.0) / 2.0);
                        if theme::danger_button(ui, "Удалить").clicked() {
                            self.start(&ctx);
                        }
                        if ui.button("Отмена").clicked() {
                            close(ui);
                        }
                    });
                });
            }
        }
    }
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(14.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Harbor").size(26.0).strong().color(theme::ACCENT));
                ui.label(RichText::new(format!("версия {}", env!("CARGO_PKG_VERSION"))).color(theme::SUBTEXT));
            });
            ui.add_space(14.0);
            self.body(ui);
        });
    }
}
