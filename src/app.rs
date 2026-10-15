use crate::github::{self, Client, EtagCache, Progress, SyncResult, Target};
use crate::model::{self, Installed, Repo, SOURCE_TAG, State, ago_iso, ago_secs, clock, human_size, now, repo_dir};
use crate::theme;
use crate::update;
use eframe::egui::{self, Color32, RichText, Ui};
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const NEW_DAYS: i64 = 3;

enum Msg {
    Synced(Result<SyncResult, String>),
    Downloaded {
        key: String,
        result: Result<Installed, String>,
    },
    SelfUpdated(Result<(), String>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum View {
    Repos,
    Log,
    Settings,
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    WithRelease,
    Downloaded,
    Updates,
    Favorites,
    Private,
    Hidden,
}

impl Filter {
    const ALL: [Filter; 7] = [
        Filter::All,
        Filter::WithRelease,
        Filter::Downloaded,
        Filter::Updates,
        Filter::Favorites,
        Filter::Private,
        Filter::Hidden,
    ];
    fn label(self) -> &'static str {
        match self {
            Filter::All => "Все",
            Filter::WithRelease => "С релизом",
            Filter::Downloaded => "Скачанные",
            Filter::Updates => "Есть обновление",
            Filter::Favorites => "Избранные",
            Filter::Private => "Приватные",
            Filter::Hidden => "Скрытые",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Updated,
    Name,
    Stars,
    Release,
    Size,
}

impl Sort {
    const ALL: [Sort; 5] = [Sort::Updated, Sort::Name, Sort::Stars, Sort::Release, Sort::Size];
    fn label(self) -> &'static str {
        match self {
            Sort::Updated => "По активности",
            Sort::Name => "По имени",
            Sort::Stars => "По звёздам",
            Sort::Release => "По дате релиза",
            Sort::Size => "По размеру",
        }
    }
}

enum Confirm {
    DeleteFiles(String),
    WipeAll,
}

enum Action {
    Download(String, Target),
    Cancel(String),
    Run(String),
    Reveal(String),
    Delete(String),
    Hide(String),
    Unhide(String),
    RemoveManual(String),
    Fav(String),
    SelectAsset(String, usize),
    Open(String),
    Copy(String, String),
}

struct Shot {
    path: std::path::PathBuf,
    frames: u32,
    asked: bool,
    settled: u32,
    started: Instant,
}

pub struct App {
    st: State,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    etags: EtagCache,
    rate: Arc<AtomicI64>,
    syncing: bool,
    last_try: Option<Instant>,
    sync_error: Option<String>,
    warnings: Vec<String>,
    token_source: &'static str,
    downloads: HashMap<String, Arc<Progress>>,
    asset_sel: HashMap<String, usize>,
    view: View,
    search: String,
    filter: Filter,
    sort: Sort,
    confirm: Option<Confirm>,
    add_text: String,
    show_add: bool,
    toast: Option<(String, bool, Instant)>,
    update: Option<update::Available>,
    updating: Option<Arc<Progress>>,
    shot: Option<Shot>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, shot: Option<(std::path::PathBuf, View)>) -> Self {
        theme::install(&cc.egui_ctx);
        let mut st = model::load();
        st.installed.retain(|_, i| i.path.exists());
        let (tx, rx) = channel();
        let (view, shot) = match shot {
            Some((path, view)) => (
                view,
                Some(Shot {
                    path,
                    frames: 0,
                    asked: false,
                    settled: 0,
                    started: Instant::now(),
                }),
            ),
            None => (View::Repos, None),
        };
        Self {
            st,
            tx,
            rx,
            etags: Arc::new(Mutex::new(HashMap::new())),
            rate: Arc::new(AtomicI64::new(-1)),
            syncing: false,
            last_try: None,
            sync_error: None,
            warnings: Vec::new(),
            token_source: "—",
            downloads: HashMap::new(),
            asset_sel: HashMap::new(),
            view,
            search: String::new(),
            filter: Filter::All,
            sort: Sort::Updated,
            confirm: None,
            add_text: String::new(),
            show_add: false,
            toast: None,
            update: None,
            updating: None,
            shot,
        }
    }

    fn save(&self) {
        model::save(&self.st);
    }

    fn say(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        self.st.push_log(if error {
            format!("Ошибка: {text}")
        } else {
            text.clone()
        });
        self.toast = Some((text, error, Instant::now()));
        self.save();
    }

    // ---------- background work ----------

    fn start_sync(&mut self, ctx: &egui::Context) {
        if self.syncing {
            return;
        }
        self.syncing = true;
        self.last_try = Some(Instant::now());
        let settings = self.st.settings.clone();
        let manual = self.st.manual.clone();
        let prev = self.st.repos.clone();
        let (tx, ctx, etags, rate) = (self.tx.clone(), ctx.clone(), self.etags.clone(), self.rate.clone());
        std::thread::spawn(move || {
            let (token, _) = github::resolve_token(&settings);
            let client = Client::new(token, etags, rate);
            let _ = tx.send(Msg::Synced(github::sync(&client, &settings, &manual, &prev)));
            ctx.request_repaint();
        });
    }

    fn start_download(&mut self, ctx: &egui::Context, key: &str, target: Target) {
        if self.downloads.contains_key(key) {
            return;
        }
        let Some(repo) = self.st.repos.iter().find(|r| r.key() == key).cloned() else {
            return;
        };
        let prog = Arc::new(Progress::default());
        self.downloads.insert(key.to_string(), prog.clone());
        let settings = self.st.settings.clone();
        let (tx, ctx, etags, rate, key) = (
            self.tx.clone(),
            ctx.clone(),
            self.etags.clone(),
            self.rate.clone(),
            key.to_string(),
        );
        std::thread::spawn(move || {
            let (token, _) = github::resolve_token(&settings);
            let client = Client::new(token, etags, rate);
            let result = github::download(&client, &repo, target, &settings, &prog);
            let _ = tx.send(Msg::Downloaded { key, result });
            ctx.request_repaint();
        });
    }

    fn pick_asset(&self, repo: &Repo) -> Option<usize> {
        let rel = repo.release.as_ref()?;
        let sel = self
            .asset_sel
            .get(&repo.key())
            .copied()
            .filter(|&i| i < rel.assets.len());
        sel.or_else(|| {
            let prefer = self.st.installed.get(&repo.key()).map(|i| i.asset.clone());
            prefer.and_then(|n| rel.assets.iter().position(|a| a.name == n))
        })
        .or_else(|| rel.best_asset())
    }

    fn start_self_update(&mut self, ctx: &egui::Context) {
        let Some(repo) = self
            .st
            .repos
            .iter()
            .find(|r| r.owner.eq_ignore_ascii_case(update::OWNER) && r.name == update::REPO)
            .cloned()
        else {
            return;
        };
        if self.updating.is_some() {
            return;
        }
        let prog = Arc::new(Progress::default());
        self.updating = Some(prog.clone());
        let settings = self.st.settings.clone();
        let (tx, ctx, etags, rate) = (self.tx.clone(), ctx.clone(), self.etags.clone(), self.rate.clone());
        std::thread::spawn(move || {
            let (token, _) = github::resolve_token(&settings);
            let client = Client::new(token, etags, rate);
            let _ = tx.send(Msg::SelfUpdated(update::apply(&client, &repo, &prog)));
            ctx.request_repaint();
        });
    }

    fn update_banner(&mut self, ui: &mut Ui) {
        let Some(av) = &self.update else { return };
        let tag = av.tag.clone();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "Доступна новая версия Harbor {tag} (у тебя {})",
                    env!("CARGO_PKG_VERSION")
                ))
                .color(theme::WARN),
            );
            if let Some(p) = &self.updating {
                let (done, total) = (p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
                let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                ui.add(egui::ProgressBar::new(frac).desired_width(180.0).text(p.stage_text()));
            } else if theme::primary_button(ui, "Обновить Harbor").clicked() {
                self.start_self_update(&ui.ctx().clone());
            }
        });
    }

    fn update_all(&mut self, ctx: &egui::Context) {
        let todo: Vec<(String, usize)> = self
            .st
            .repos
            .iter()
            .filter(|r| self.st.update_available(r))
            .filter_map(|r| self.pick_asset(r).map(|i| (r.key(), i)))
            .collect();
        for (key, i) in todo {
            self.start_download(ctx, &key, Target::Asset(i));
        }
    }

    fn pump(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Synced(res) => self.on_synced(ctx, res),
                Msg::SelfUpdated(res) => {
                    self.updating = None;
                    match res {
                        Ok(()) => {
                            if let Ok(exe) = std::env::current_exe() {
                                let _ = std::process::Command::new(exe).spawn();
                            }
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => self.say(format!("Обновление Harbor: {e}"), true),
                    }
                }
                Msg::Downloaded { key, result } => {
                    self.downloads.remove(&key);
                    match result {
                        Ok(inst) => {
                            let text = format!("Скачано: {key} {} ({})", inst.tag, human_size(inst.size));
                            self.st.installed.insert(key, inst);
                            self.say(text, false);
                        }
                        Err(e) if e == "отменено" => self.say(format!("Загрузка {key} отменена"), false),
                        Err(e) => self.say(format!("{key}: {e}"), true),
                    }
                }
            }
        }
        let interval = Duration::from_secs(60 * u64::from(self.st.settings.refresh_minutes.max(1)));
        if !self.syncing && self.last_try.is_none_or(|t| t.elapsed() >= interval) {
            self.start_sync(ctx);
        }
        ctx.request_repaint_after(if self.downloads.is_empty() && self.updating.is_none() {
            Duration::from_secs(15)
        } else {
            Duration::from_millis(120)
        });
    }

    fn on_synced(&mut self, ctx: &egui::Context, res: Result<SyncResult, String>) {
        self.syncing = false;
        match res {
            Err(e) => self.sync_error = Some(e),
            Ok(r) => {
                self.sync_error = None;
                self.warnings = r.warnings;
                self.token_source = r.token_source;
                let first_run = self.st.first_seen.is_empty();
                let mut logs = Vec::new();
                for repo in &r.repos {
                    let key = repo.key();
                    if !self.st.first_seen.contains_key(&key) {
                        self.st
                            .first_seen
                            .insert(key.clone(), if first_run { 0 } else { now() });
                        if !first_run {
                            logs.push(format!("Новый репозиторий: {key}"));
                        }
                    }
                    let old = self
                        .st
                        .repos
                        .iter()
                        .find(|o| o.key() == key)
                        .and_then(|o| o.release.as_ref().map(|x| x.tag.clone()));
                    if let (Some(old), Some(new)) = (old, &repo.release)
                        && old != new.tag
                    {
                        logs.push(format!("Новый релиз {key}: {old} → {}", new.tag));
                    }
                }
                for l in logs {
                    self.st.push_log(l);
                }
                self.st.repos = r.repos;
                if self.shot.is_some() {
                    self.st.repos.retain(|x| !x.private);
                }
                self.update = update::find(&self.st.repos);
                self.st.installed.retain(|_, i| i.path.exists());
                self.st.last_sync = now();
                self.save();
                if self.st.settings.auto_update {
                    self.update_all(ctx);
                }
            }
        }
    }

    // ---------- actions ----------

    fn apply(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Download(key, t) => self.start_download(ctx, &key, t),
                Action::Cancel(key) => {
                    if let Some(p) = self.downloads.get(&key) {
                        p.cancel.store(true, Ordering::Relaxed);
                    }
                }
                Action::Run(key) => {
                    if let Some(i) = self.st.installed.get(&key).cloned() {
                        if i.path.exists() {
                            shell_open(&i.path);
                        } else {
                            self.st.installed.remove(&key);
                            self.say(format!("{key}: файл больше не существует"), true);
                        }
                    }
                }
                Action::Reveal(key) => {
                    let dir = self.st.installed.get(&key).map(|i| i.path.clone());
                    match dir {
                        Some(p) if p.exists() => reveal(&p),
                        _ => {
                            let name = key.rsplit('/').next().unwrap_or(&key);
                            let d = repo_dir(&self.st.settings.download_dir, name);
                            let _ = std::fs::create_dir_all(&d);
                            shell_open(&d);
                        }
                    }
                }
                Action::Delete(key) => self.confirm = Some(Confirm::DeleteFiles(key)),
                Action::Hide(key) => {
                    self.st.hidden.insert(key.clone());
                    self.say(format!("«{key}» убран из списка (фильтр «Скрытые» вернёт его)"), false);
                }
                Action::Unhide(key) => {
                    self.st.hidden.remove(&key);
                    self.say(format!("«{key}» возвращён в список"), false);
                }
                Action::RemoveManual(key) => {
                    self.st.manual.remove(&key);
                    self.st.repos.retain(|r| r.key() != key);
                    self.say(format!("«{key}» удалён из списка"), false);
                }
                Action::Fav(key) => {
                    if !self.st.favorites.remove(&key) {
                        self.st.favorites.insert(key);
                    }
                    self.save();
                }
                Action::SelectAsset(key, i) => {
                    self.asset_sel.insert(key, i);
                }
                Action::Open(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
                Action::Copy(text, note) => {
                    ctx.copy_text(text);
                    self.toast = Some((note, false, Instant::now()));
                }
            }
        }
    }

    fn delete_files(&mut self, key: &str) {
        let name = key.rsplit('/').next().unwrap_or(key);
        let root = self.st.settings.download_dir.clone();
        let dir = repo_dir(&root, name);
        if dir.starts_with(&root) && dir != root && dir.exists() {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {
                    self.st.installed.remove(key);
                    self.say(format!("Файлы {key} удалены с диска"), false);
                }
                Err(e) => self.say(format!("не удалось удалить {}: {e}", dir.display()), true),
            }
        } else {
            self.st.installed.remove(key);
            self.save();
        }
    }

    fn add_manual(&mut self, ctx: &egui::Context) {
        match model::parse_repo_ref(&self.add_text) {
            Some((o, n)) => {
                let key = format!("{o}/{n}");
                if self.st.repos.iter().any(|r| r.key().eq_ignore_ascii_case(&key)) {
                    self.st.hidden.remove(&key);
                    self.say(format!("{key} уже в списке"), false);
                } else {
                    self.st.manual.insert(key.clone());
                    self.say(format!("Добавлен {key}, загружаю…"), false);
                    self.start_sync(ctx);
                }
                self.add_text.clear();
                self.show_add = false;
            }
            None => self.say("Не похоже на репозиторий: нужно owner/name или ссылка на GitHub", true),
        }
    }

    fn markdown_list(&self) -> String {
        let mut out = String::from("# Мои репозитории\n\n");
        for r in self
            .st
            .repos
            .iter()
            .filter(|r| !r.private && !self.st.hidden.contains(&r.key()))
        {
            let ver = r.release.as_ref().map(|x| format!(" `{}`", x.tag)).unwrap_or_default();
            out += &format!("- [{}]({}){} — {}\n", r.name, r.html_url, ver, r.description);
        }
        out
    }

    // ---------- view model ----------

    fn visible(&self) -> Vec<usize> {
        let q = self.search.trim().to_lowercase();
        let mut v: Vec<usize> = self
            .st
            .repos
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                let key = r.key();
                if (self.filter == Filter::Hidden) != self.st.hidden.contains(&key) {
                    return false;
                }
                let pass = match self.filter {
                    Filter::All | Filter::Hidden => true,
                    Filter::WithRelease => r.release.is_some(),
                    Filter::Downloaded => self.st.installed.contains_key(&key),
                    Filter::Updates => self.st.update_available(r),
                    Filter::Favorites => self.st.favorites.contains(&key),
                    Filter::Private => r.private,
                };
                pass && (q.is_empty()
                    || r.name.to_lowercase().contains(&q)
                    || r.description.to_lowercase().contains(&q)
                    || r.language.as_deref().is_some_and(|l| l.to_lowercase().contains(&q)))
            })
            .map(|(i, _)| i)
            .collect();
        let repos = &self.st.repos;
        let rel_size = |r: &Repo| {
            r.release
                .as_ref()
                .and_then(|x| x.best_asset().map(|i| x.assets[i].size))
                .unwrap_or(0)
        };
        v.sort_by(|&a, &b| {
            let (ra, rb) = (&repos[a], &repos[b]);
            let fav = |r: &Repo| self.st.favorites.contains(&r.key());
            fav(rb).cmp(&fav(ra)).then_with(|| match self.sort {
                Sort::Updated => rb.pushed_at.cmp(&ra.pushed_at),
                Sort::Name => ra.name.to_lowercase().cmp(&rb.name.to_lowercase()),
                Sort::Stars => rb.stars.cmp(&ra.stars),
                Sort::Release => {
                    let d = |r: &Repo| r.release.as_ref().map(|x| x.published_at.clone()).unwrap_or_default();
                    d(rb).cmp(&d(ra))
                }
                Sort::Size => rel_size(rb).cmp(&rel_size(ra)),
            })
        });
        v
    }

    // ---------- UI ----------

    fn top_bar(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let updates = self.st.repos.iter().filter(|r| self.st.update_available(r)).count();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Harbor").size(22.0).strong().color(theme::ACCENT));
            ui.add_space(12.0);
            ui.selectable_value(&mut self.view, View::Repos, "Репозитории");
            ui.selectable_value(&mut self.view, View::Log, "Журнал");
            ui.selectable_value(&mut self.view, View::Settings, "Настройки");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if self.syncing {
                    "Синхронизация…"
                } else {
                    "Обновить список"
                };
                if ui
                    .add_enabled(!self.syncing, egui::Button::new(label))
                    .on_hover_text("F5")
                    .clicked()
                {
                    self.start_sync(ctx);
                }
                if ui.button("+ Добавить репозиторий").clicked() {
                    self.show_add = true;
                }
                if updates > 0 && theme::primary_button(ui, &format!("Обновить всё ({updates})")).clicked() {
                    self.update_all(ctx);
                }
            });
        });
        if self.view == View::Repos {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .id(egui::Id::new("search"))
                        .hint_text("Поиск по названию, описанию, языку (Ctrl+F)")
                        .desired_width(300.0),
                );
                if !self.search.is_empty() && ui.small_button("×").clicked() {
                    self.search.clear();
                }
                egui::ComboBox::from_id_salt("filter")
                    .selected_text(self.filter.label())
                    .show_ui(ui, |ui| {
                        for f in Filter::ALL {
                            ui.selectable_value(&mut self.filter, f, f.label());
                        }
                    });
                egui::ComboBox::from_id_salt("sort")
                    .selected_text(self.sort.label())
                    .show_ui(ui, |ui| {
                        for s in Sort::ALL {
                            ui.selectable_value(&mut self.sort, s, s.label());
                        }
                    });
                if ui.selectable_label(!self.st.settings.list_mode, "Плитки").clicked() {
                    self.st.settings.list_mode = false;
                    self.save();
                }
                if ui.selectable_label(self.st.settings.list_mode, "Список").clicked() {
                    self.st.settings.list_mode = true;
                    self.save();
                }
            });
            ui.horizontal(|ui| {
                let total = self
                    .st
                    .repos
                    .iter()
                    .filter(|r| !self.st.hidden.contains(&r.key()))
                    .count();
                let disk: u64 = self.st.installed.values().map(|i| i.size).sum();
                let stat = |ui: &mut Ui, t: String, c: Color32| {
                    ui.label(RichText::new(t).color(c));
                };
                stat(ui, format!("Репозиториев: {total}"), theme::SUBTEXT);
                stat(
                    ui,
                    format!("Скачано: {} ({})", self.st.installed.len(), human_size(disk)),
                    theme::GOOD,
                );
                if updates > 0 {
                    stat(ui, format!("Обновлений: {updates}"), theme::WARN);
                }
                if !self.st.hidden.is_empty() {
                    stat(ui, format!("Скрыто: {}", self.st.hidden.len()), theme::SUBTEXT);
                }
            });
        }
        ui.add_space(4.0);
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if self.syncing {
                ui.spinner();
                ui.label("Синхронизация с GitHub…");
            } else if let Some(e) = &self.sync_error {
                ui.label(RichText::new(format!("Ошибка синхронизации: {e}")).color(theme::BAD));
            } else if self.st.last_sync > 0 {
                let rate = self.rate.load(Ordering::Relaxed);
                let rate = if rate >= 0 {
                    format!(" · лимит API: {rate}")
                } else {
                    String::new()
                };
                ui.label(
                    RichText::new(format!(
                        "Обновлено {} · токен: {}{rate}",
                        ago_secs(now() - self.st.last_sync),
                        self.token_source
                    ))
                    .color(theme::SUBTEXT),
                );
            }
            if !self.warnings.is_empty() {
                ui.label(RichText::new(format!("предупреждений: {}", self.warnings.len())).color(theme::WARN))
                    .on_hover_text(self.warnings.join("\n"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((t, err, at)) = &self.toast
                    && at.elapsed() < Duration::from_secs(8)
                {
                    ui.label(RichText::new(t).color(if *err { theme::BAD } else { theme::GOOD }));
                }
                if !self.downloads.is_empty() {
                    ui.label(RichText::new(format!("загрузок: {}", self.downloads.len())).color(theme::BLUE));
                }
            });
        });
    }

    fn repos_view(&mut self, ui: &mut Ui) -> Vec<Action> {
        let vis = self.visible();
        let mut actions = Vec::new();
        if vis.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                let msg = if self.syncing && self.st.repos.is_empty() {
                    "Загружаю список репозиториев…"
                } else if let (Some(e), true) = (&self.sync_error, self.st.repos.is_empty()) {
                    e.as_str()
                } else {
                    "Ничего не найдено — измени поиск или фильтр"
                };
                ui.label(RichText::new(msg).size(16.0).color(theme::SUBTEXT));
            });
            return actions;
        }
        let list = self.st.settings.list_mode;
        let this = &*self;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let gap = ui.spacing().item_spacing.x;
            let cols = if list {
                1
            } else {
                (((ui.available_width() + gap) / (400.0 + gap)).floor() as usize).max(1)
            };
            for chunk in vis.chunks(cols) {
                ui.columns(cols, |c| {
                    for (i, &idx) in chunk.iter().enumerate() {
                        this.card(&mut c[i], &this.st.repos[idx], &mut actions);
                    }
                });
                ui.add_space(4.0);
            }
        });
        actions
    }

    fn card(&self, ui: &mut Ui, repo: &Repo, actions: &mut Vec<Action>) {
        let key = repo.key();
        let installed = self.st.installed.get(&key);
        let upd = self.st.update_available(repo);
        let hidden = self.st.hidden.contains(&key);
        let fav = self.st.favorites.contains(&key);
        let is_new = self
            .st
            .first_seen
            .get(&key)
            .is_some_and(|&t| t > 0 && now() - t < NEW_DAYS * 86_400);
        theme::card(upd).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(if self.st.settings.list_mode { 0.0 } else { 236.0 });
            ui.horizontal_wrapped(|ui| {
                let title =
                    ui.add(egui::Label::new(RichText::new(&repo.name).size(18.0).strong()).sense(egui::Sense::click()));
                if title.on_hover_text("Открыть на GitHub").clicked() {
                    actions.push(Action::Open(repo.html_url.clone()));
                }
                if repo.private {
                    theme::badge(ui, "ПРИВАТНЫЙ", theme::WARN);
                }
                if repo.archived {
                    theme::badge(ui, "АРХИВ", theme::SUBTEXT);
                }
                if repo.fork {
                    theme::badge(ui, "ФОРК", theme::SUBTEXT);
                }
                if repo.manual {
                    theme::badge(ui, "ДОБАВЛЕН", theme::BLUE);
                }
                if is_new {
                    theme::badge(ui, "NEW", theme::GOOD);
                }
                let star = if fav { "★" } else { "☆" };
                if ui
                    .add(
                        egui::Button::new(RichText::new(star).color(if fav { theme::WARN } else { theme::SUBTEXT }))
                            .frame(false),
                    )
                    .on_hover_text("Избранное")
                    .clicked()
                {
                    actions.push(Action::Fav(key.clone()));
                }
            });
            let desc = if repo.description.is_empty() {
                "Без описания".to_string()
            } else {
                truncate(&repo.description, 150)
            };
            ui.label(RichText::new(desc).color(theme::SUBTEXT));
            ui.horizontal_wrapped(|ui| {
                if let Some(l) = &repo.language {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, theme::lang_color(l));
                    ui.label(RichText::new(l).small());
                }
                if repo.stars > 0 {
                    ui.label(RichText::new(format!("★ {}", repo.stars)).small());
                }
                ui.label(
                    RichText::new(format!("активность: {}", ago_iso(&repo.pushed_at)))
                        .small()
                        .color(theme::SUBTEXT),
                );
            });
            match &repo.release {
                Some(rel) => {
                    let size = self
                        .pick_asset(repo)
                        .map(|i| format!(" · {}", human_size(rel.assets[i].size)))
                        .unwrap_or_default();
                    ui.label(
                        RichText::new(format!("Релиз {} · {}{size}", rel.tag, ago_iso(&rel.published_at)))
                            .small()
                            .color(theme::BLUE),
                    );
                }
                None => {
                    ui.label(
                        RichText::new("Релизов нет — доступны только исходники")
                            .small()
                            .color(theme::SUBTEXT),
                    );
                }
            }
            if let Some(i) = installed {
                let (text, color) = if upd {
                    let new = repo.release.as_ref().map(|r| r.tag.as_str()).unwrap_or("");
                    (format!("Скачано {} → доступно {new}", i.tag), theme::WARN)
                } else if i.tag == SOURCE_TAG {
                    (format!("Скачаны исходники · {}", human_size(i.size)), theme::GOOD)
                } else {
                    (format!("Скачано {} · {}", i.tag, human_size(i.size)), theme::GOOD)
                };
                ui.label(RichText::new(text).small().strong().color(color));
            }
            if let Some(rel) = &repo.release
                && !rel.notes.trim().is_empty()
            {
                egui::CollapsingHeader::new(RichText::new("Что нового").small())
                    .id_salt(("notes", &key))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(truncate(rel.notes.trim(), 900))
                                .small()
                                .color(theme::SUBTEXT),
                        );
                    });
            }
            ui.add_space(4.0);
            if let Some(p) = self.downloads.get(&key) {
                let (done, total) = (p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
                ui.horizontal(|ui| {
                    let text = if total > 0 {
                        format!("{} / {}", human_size(done), human_size(total))
                    } else {
                        p.stage_text()
                    };
                    let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .text(text)
                            .desired_width((ui.available_width() - 90.0).max(80.0)),
                    );
                    if ui.button("Отмена").clicked() {
                        actions.push(Action::Cancel(key.clone()));
                    }
                });
                return;
            }
            self.card_buttons(ui, repo, installed, upd, hidden, actions);
        });
    }

    fn card_buttons(
        &self,
        ui: &mut Ui,
        repo: &Repo,
        installed: Option<&Installed>,
        upd: bool,
        hidden: bool,
        actions: &mut Vec<Action>,
    ) {
        let key = repo.key();
        ui.horizontal_wrapped(|ui| {
            let assets = repo.release.as_ref().map(|r| r.downloadable()).unwrap_or_default();
            let have_file = installed.is_some();
            if let Some(rel) = &repo.release
                && !assets.is_empty()
            {
                let label = if upd {
                    format!("Обновить до {}", rel.tag)
                } else if have_file && installed.is_some_and(|i| i.tag != SOURCE_TAG) {
                    "Скачать заново".to_string()
                } else {
                    format!("Скачать {}", rel.tag)
                };
                let primary = !have_file || upd;
                let clicked = if primary {
                    theme::primary_button(ui, &label).clicked()
                } else {
                    ui.button(&label).clicked()
                };
                if clicked && let Some(i) = self.pick_asset(repo) {
                    actions.push(Action::Download(key.clone(), Target::Asset(i)));
                }
                if assets.len() > 1 {
                    let cur = self.pick_asset(repo).unwrap_or(assets[0]);
                    egui::ComboBox::from_id_salt(("asset", &key))
                        .width(150.0)
                        .selected_text(truncate(&rel.assets[cur].name, 22))
                        .show_ui(ui, |ui| {
                            for &i in &assets {
                                let a = &rel.assets[i];
                                if ui
                                    .selectable_label(i == cur, format!("{} ({})", a.name, human_size(a.size)))
                                    .clicked()
                                {
                                    actions.push(Action::SelectAsset(key.clone(), i));
                                }
                            }
                        });
                }
            } else if !have_file && theme::primary_button(ui, "Скачать исходники").clicked() {
                actions.push(Action::Download(key.clone(), Target::Source));
            }
            if let Some(i) = installed {
                let ext = i.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                let run = if matches!(ext.as_str(), "exe" | "msi") {
                    "Запустить"
                } else {
                    "Открыть"
                };
                if ui.button(run).clicked() {
                    actions.push(Action::Run(key.clone()));
                }
                if ui.button("Папка").clicked() {
                    actions.push(Action::Reveal(key.clone()));
                }
                if theme::danger_button(ui, "Удалить")
                    .on_hover_text("Удалить скачанные файлы с диска")
                    .clicked()
                {
                    actions.push(Action::Delete(key.clone()));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("GitHub").clicked() {
                actions.push(Action::Open(repo.html_url.clone()));
            }
            if hidden {
                if ui.small_button("Вернуть в список").clicked() {
                    actions.push(Action::Unhide(key.clone()));
                }
            } else if ui
                .small_button("Удалить из списка")
                .on_hover_text("Скрыть карточку; файлы на диске не трогает")
                .clicked()
            {
                actions.push(if repo.manual {
                    Action::RemoveManual(key.clone())
                } else {
                    Action::Hide(key.clone())
                });
            }
            ui.menu_button("Ещё", |ui| {
                if ui.button("Скачать исходники (zip)").clicked() {
                    actions.push(Action::Download(key.clone(), Target::Source));
                    ui.close();
                }
                if ui.button("Копировать ссылку").clicked() {
                    actions.push(Action::Copy(repo.html_url.clone(), "Ссылка скопирована".into()));
                    ui.close();
                }
                if ui.button("Копировать git clone").clicked() {
                    actions.push(Action::Copy(
                        format!("git clone {}.git", repo.html_url),
                        "Команда скопирована".into(),
                    ));
                    ui.close();
                }
                if ui.button("Открыть релизы").clicked() {
                    actions.push(Action::Open(format!("{}/releases", repo.html_url)));
                    ui.close();
                }
            });
        });
    }

    fn log_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.heading("Журнал");
            if ui.button("Очистить").clicked() {
                self.st.log.clear();
                self.save();
            }
        });
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if self.st.log.is_empty() {
                ui.label(RichText::new("Пока пусто").color(theme::SUBTEXT));
            }
            for e in self.st.log.iter().rev() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(clock(e.at)).monospace().color(theme::SUBTEXT));
                    let c = if e.text.starts_with("Ошибка") {
                        theme::BAD
                    } else {
                        theme::TEXT
                    };
                    ui.label(RichText::new(&e.text).color(c));
                });
            }
        });
    }

    fn settings_view(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let mut changed = false;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.heading("Аккаунт GitHub");
            ui.horizontal(|ui| {
                ui.label("Логин:");
                changed |= ui.text_edit_singleline(&mut self.st.settings.username).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Токен (необязательно):");
                changed |= ui.add(egui::TextEdit::singleline(&mut self.st.settings.token).password(true).desired_width(300.0)).changed();
            });
            changed |= ui.checkbox(&mut self.st.settings.use_gh_token, "Если токена нет — взять из gh CLI (gh auth token)").changed();
            ui.label(
                RichText::new(format!(
                    "Сейчас токен: {}. Без токена лимит GitHub — 60 запросов/час и нет приватных репозиториев; с токеном — 5000/час.",
                    self.token_source
                ))
                .small()
                .color(theme::SUBTEXT),
            );
            ui.add_space(10.0);
            ui.heading("Список");
            changed |= ui.checkbox(&mut self.st.settings.include_private, "Показывать приватные (нужен токен)").changed();
            changed |= ui.checkbox(&mut self.st.settings.include_forks, "Показывать форки").changed();
            changed |= ui.checkbox(&mut self.st.settings.include_archived, "Показывать архивные").changed();
            ui.horizontal(|ui| {
                ui.label("Автообновление списка, минут:");
                changed |= ui.add(egui::Slider::new(&mut self.st.settings.refresh_minutes, 1..=120)).changed();
            });
            ui.add_space(10.0);
            ui.heading("Загрузки");
            ui.horizontal(|ui| {
                ui.label("Папка:");
                let mut dir = self.st.settings.download_dir.to_string_lossy().to_string();
                if ui.add(egui::TextEdit::singleline(&mut dir).desired_width(420.0)).changed() {
                    self.st.settings.download_dir = dir.into();
                    changed = true;
                }
                if ui.button("Выбрать…").clicked()
                    && let Some(p) = rfd::FileDialog::new().set_directory(&self.st.settings.download_dir).pick_folder()
                {
                    self.st.settings.download_dir = p;
                    changed = true;
                }
                if ui.button("Открыть").clicked() {
                    let _ = std::fs::create_dir_all(&self.st.settings.download_dir);
                    shell_open(&self.st.settings.download_dir);
                }
            });
            changed |= ui.checkbox(&mut self.st.settings.delete_old, "После обновления удалять старые версии").changed();
            changed |= ui.checkbox(&mut self.st.settings.auto_update, "Автоматически скачивать обновления уже скачанных программ").changed();
            ui.label(RichText::new("Проверка SHA-256 выполняется всегда, если в релизе есть файл контрольных сумм.").small().color(theme::SUBTEXT));
            ui.add_space(10.0);
            ui.heading("Сервис");
            ui.horizontal(|ui| {
                if ui.button("Скопировать список репозиториев (Markdown)").clicked() {
                    ctx.copy_text(self.markdown_list());
                    self.toast = Some(("Список скопирован".into(), false, Instant::now()));
                }
                if ui.button("Сбросить кэш GitHub").clicked() {
                    if let Ok(mut c) = self.etags.lock() {
                        c.clear();
                    }
                    self.start_sync(ctx);
                }
                if ui.button("Вернуть все скрытые").clicked() {
                    self.st.hidden.clear();
                    changed = true;
                }
                if theme::danger_button(ui, "Удалить все скачанные файлы").clicked() {
                    self.confirm = Some(Confirm::WipeAll);
                }
            });
        });
        if changed {
            self.save();
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.show_add {
            let mut open = true;
            let mut submit = false;
            egui::Window::new("Добавить репозиторий")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("owner/name или ссылка на GitHub (любой публичный репозиторий):");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.add_text)
                            .desired_width(360.0)
                            .hint_text("https://github.com/owner/repo"),
                    );
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                    if theme::primary_button(ui, "Добавить").clicked() {
                        submit = true;
                    }
                });
            if submit {
                self.add_manual(ctx);
            }
            if !open {
                self.show_add = false;
            }
        }
        let Some(c) = &self.confirm else { return };
        let (title, text) = match c {
            Confirm::DeleteFiles(k) => {
                let name = k.rsplit('/').next().unwrap_or(k);
                let dir = repo_dir(&self.st.settings.download_dir, name);
                (
                    "Удалить файлы?",
                    format!(
                        "С диска будет удалена папка:\n{}\n\nИз списка репозиторий не пропадёт.",
                        dir.display()
                    ),
                )
            }
            Confirm::WipeAll => (
                "Удалить всё скачанное?",
                format!(
                    "Будут удалены папки скачанных программ ({} шт.).",
                    self.st.installed.len()
                ),
            ),
        };
        let (mut yes, mut no) = (false, false);
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(text);
                ui.horizontal(|ui| {
                    yes = theme::danger_button(ui, "Удалить").clicked();
                    no = ui.button("Отмена").clicked();
                });
            });
        if yes {
            match self.confirm.take() {
                Some(Confirm::DeleteFiles(k)) => self.delete_files(&k),
                Some(Confirm::WipeAll) => {
                    let keys: Vec<String> = self.st.installed.keys().cloned().collect();
                    for k in keys {
                        self.delete_files(&k);
                    }
                }
                None => {}
            }
        } else if no {
            self.confirm = None;
        }
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(s) = &mut self.shot else { return };
        s.frames += 1;
        if (!self.syncing && s.frames > 25) || s.started.elapsed() > Duration::from_secs(90) {
            s.settled += 1;
        }
        let img = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = img {
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
            let ok = image::save_buffer(
                &s.path,
                &rgba,
                img.size[0] as u32,
                img.size[1] as u32,
                image::ColorType::Rgba8,
            )
            .is_ok();
            println!("{} {}", if ok { "saved" } else { "FAILED" }, s.path.display());
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if s.settled > 15 && !s.asked {
            s.asked = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        ctx.request_repaint();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump(&ctx);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::F)) {
            self.view = View::Repos;
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("search")));
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F5)) {
            self.start_sync(&ctx);
        }
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui, &ctx));
        if self.update.is_some() {
            egui::Panel::top("update_banner").show(ui, |ui| self.update_banner(ui));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        let mut actions = Vec::new();
        egui::CentralPanel::default().show(ui, |ui| match self.view {
            View::Repos => actions = self.repos_view(ui),
            View::Log => self.log_view(ui),
            View::Settings => self.settings_view(ui, &ctx),
        });
        self.apply(&ctx, actions);
        self.dialogs(&ctx);
        self.screenshot_step(&ctx);
    }
}

fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace(['\r', '\n'], " ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

fn shell_open(path: &Path) {
    let _ = std::process::Command::new("explorer.exe")
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

fn reveal(path: &Path) {
    let _ = std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
