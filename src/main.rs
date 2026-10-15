#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod github;
mod installer;
mod model;
mod scan;
mod theme;
mod update;
mod winint;

use eframe::egui;

fn icon() -> egui::IconData {
    const N: usize = 64;
    let mut rgba = vec![0u8; N * N * 4];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            // rounded square with a vertical gradient and a white hub node in the middle
            let (ax, ay) = (dx.abs() - 22.0, dy.abs() - 22.0);
            let d = (ax.max(0.0).powi(2) + ay.max(0.0).powi(2)).sqrt() - 8.0 + ax.max(ay).min(0.0);
            let i = (y * N + x) * 4;
            if d < 0.0 {
                let t = y as f32 / N as f32;
                let node = (dx * dx + dy * dy).sqrt() < 11.0;
                let ring = (dx * dx + dy * dy).sqrt() < 21.0 && (dx * dx + dy * dy).sqrt() > 17.0;
                let px = if node || ring {
                    [255, 255, 255]
                } else {
                    [(203.0 - 60.0 * t) as u8, (166.0 - 20.0 * t) as u8, 247]
                };
                rgba[i..i + 3].copy_from_slice(&px);
                rgba[i + 3] = 255;
            }
        }
    }
    egui::IconData {
        rgba,
        width: N as u32,
        height: N as u32,
    }
}

fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    let mut shot = None;
    let mut setup = if winint::is_setup_launch() {
        Some(installer::Kind::Install)
    } else {
        None
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--install" => setup = Some(installer::Kind::Install),
            "--uninstall" => setup = Some(installer::Kind::Uninstall),
            "--shot" => {
                let path = args.next().unwrap_or_else(|| "harbor.png".into());
                let view = match args.next().as_deref() {
                    Some("settings") => app::View::Settings,
                    Some("log") => app::View::Log,
                    _ => app::View::Repos,
                };
                shot = Some((path.into(), view));
            }
            _ => {}
        }
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Harbor")
        .with_app_id("Harbor")
        .with_icon(icon());
    if let Some(kind) = setup {
        viewport = viewport
            .with_inner_size([520.0, 330.0])
            .with_resizable(false)
            .with_maximize_button(false);
        return eframe::run_native(
            "Harbor",
            eframe::NativeOptions {
                viewport,
                ..Default::default()
            },
            Box::new(move |cc| Ok(Box::new(installer::SetupApp::new(cc, kind)))),
        );
    }
    winint::cleanup_old();
    viewport = viewport
        .with_inner_size([1240.0, 800.0])
        .with_min_inner_size([760.0, 520.0]);
    eframe::run_native(
        "Harbor",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, shot)))),
    )
}
