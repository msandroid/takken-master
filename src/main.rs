use chrono::{Duration, Local, NaiveDate};
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

const STORAGE_KEY: &str = "takken_master_native_v1";
const CATEGORY_NAMES: [&str; 4] = ["宅建業法", "権利関係", "法令上の制限", "税・その他"];
const CATEGORY_KEYS: [&str; 4] = ["law", "rights", "reg", "other"];
const WEIGHTS: [usize; 4] = [20, 14, 8, 8];

#[derive(Clone, Deserialize)]
struct Question {
    id: u32,
    category: String,
    topic: String,
    text: String,
    options: [String; 4],
    answer: usize,
    explain: String,
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct Record {
    attempts: u32,
    correct: u32,
    repetitions: u32,
    interval_days: i64,
    next_due: Option<NaiveDate>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Attempt {
    question_id: u32,
    correct: bool,
    date: NaiveDate,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Progress {
    records: BTreeMap<u32, Record>,
    history: Vec<Attempt>,
    goal_per_day: u32,
}

impl Default for Progress {
    fn default() -> Self {
        Self { records: BTreeMap::new(), history: Vec::new(), goal_per_day: 30 }
    }
}

impl Progress {
    fn answer(&mut self, question_id: u32, correct: bool, now: NaiveDate) {
        let rec = self.records.entry(question_id).or_default();
        rec.attempts += 1;
        if correct {
            rec.correct += 1;
            rec.repetitions += 1;
            rec.interval_days = match rec.repetitions {
                1 => 1, 2 => 3, 3 => 7, 4 => 14, 5 => 30, _ => (rec.interval_days * 2).min(180),
            };
        } else {
            rec.repetitions = 0;
            rec.interval_days = 1;
        }
        rec.next_due = Some(now + Duration::days(rec.interval_days));
        self.history.push(Attempt { question_id, correct, date: now });
    }

    fn due_count(&self, questions: &[Question], now: NaiveDate) -> usize {
        questions.iter().filter(|q| self.records.get(&q.id).and_then(|r| r.next_due).is_some_and(|d| d <= now)).count()
    }

    fn today_count(&self, now: NaiveDate) -> usize {
        self.history.iter().filter(|h| h.date == now).count()
    }

    fn accuracy(&self) -> Option<f32> {
        let recent = self.history.iter().rev().take(30);
        let (mut right, mut total) = (0, 0);
        for a in recent { total += 1; if a.correct { right += 1; } }
        (total > 0).then_some(right as f32 / total as f32)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Adaptive, Unseen, Mistakes, Due }
impl Mode {
    fn label(self) -> &'static str {
        match self { Self::Adaptive => "最適化", Self::Unseen => "未解答", Self::Mistakes => "弱点", Self::Due => "期限到来" }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page { Dashboard, Study, Statistics }

struct TakkenApp {
    questions: Vec<Question>,
    progress: Progress,
    page: Page,
    mode: Mode,
    category: Option<usize>,
    session: Vec<usize>,
    cursor: usize,
    selected: Option<usize>,
    feedback: String,
    notice: String,
}

impl TakkenApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_japanese_font(&cc.egui_ctx);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let progress = cc.storage
            .and_then(|s| eframe::get_value::<Progress>(s, STORAGE_KEY))
            .unwrap_or_default();
        let questions: Vec<Question> = serde_json::from_str(include_str!("../data/questions.json"))
            .expect("bundled question JSON must be valid");
        Self { questions, progress, page: Page::Dashboard, mode: Mode::Adaptive, category: None,
               session: vec![], cursor: 0, selected: None, feedback: String::new(), notice: String::new() }
    }

    fn eligible(&self, q: &Question, now: NaiveDate) -> bool {
        if let Some(c) = self.category {
            if q.category != CATEGORY_KEYS[c] { return false; }
        }
        let record = self.progress.records.get(&q.id);
        match self.mode {
            Mode::Adaptive => true,
            Mode::Unseen => record.is_none(),
            Mode::Mistakes => record.is_some_and(|r| r.correct < r.attempts),
            Mode::Due => record.and_then(|r| r.next_due).is_some_and(|due| due <= now),
        }
    }

    fn ranking(&self, q: &Question, now: NaiveDate) -> i64 {
        let category_weight = CATEGORY_KEYS.iter().position(|c| *c == q.category).map(|i| WEIGHTS[i] as i64).unwrap_or(0);
        let Some(r) = self.progress.records.get(&q.id) else { return 10_000 + category_weight; };
        let error_ratio = if r.attempts == 0 { 0 } else { (100 * (r.attempts - r.correct) / r.attempts) as i64 };
        let overdue = r.next_due.map_or(0, |d| (now - d).num_days().clamp(0, 90));
        500 * i64::from(r.next_due.is_some_and(|d| d <= now)) + error_ratio * 10 + overdue * 5 + category_weight - r.repetitions as i64 * 8
    }

    fn start_session(&mut self) {
        let now = Local::now().date_naive();
        let mut eligible: Vec<usize> = self.questions.iter().enumerate()
            .filter(|(_, q)| self.eligible(q, now)).map(|(i, _)| i).collect();
        eligible.sort_by_key(|&i| (-self.ranking(&self.questions[i], now), self.questions[i].id));
        self.session = eligible.into_iter().take(20).collect();
        self.cursor = 0;
        self.selected = None;
        self.feedback.clear();
        self.notice = if self.session.is_empty() { "この条件に該当する問題はありません。".into() } else { String::new() };
        self.page = Page::Study;
    }

    fn submit(&mut self, chosen: usize) {
        if self.selected.is_some() || self.cursor >= self.session.len() { return; }
        let q = &self.questions[self.session[self.cursor]];
        let correct = chosen == q.answer;
        self.progress.answer(q.id, correct, Local::now().date_naive());
        self.selected = Some(chosen);
        self.feedback = if correct { "正解".into() } else { format!("不正解 — 正解は {}", q.answer + 1) };
    }

    fn next(&mut self) {
        if self.selected.is_none() { return; }
        self.cursor += 1;
        self.selected = None;
        self.feedback.clear();
        if self.cursor >= self.session.len() {
            self.notice = "演習を終了しました。結果をダッシュボードで確認できます。".into();
            self.page = Page::Dashboard;
        }
    }

    fn dashboard(&mut self, ui: &mut egui::Ui) {
        let today = Local::now().date_naive();
        ui.heading("ダッシュボード");
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            metric(ui, "総解答", format!("{} 問", self.progress.history.len()));
            metric(ui, "直近30問の正答率", self.progress.accuracy().map_or("—".into(), |a| format!("{:.0}%", a * 100.)));
            metric(ui, "復習期限到来", format!("{} 問", self.progress.due_count(&self.questions, today)));
            metric(ui, "本日の学習", format!("{} / {}", self.progress.today_count(today), self.progress.goal_per_day));
        });
        ui.add_space(14.0);
        ui.heading("学習を開始");
        ui.label("モード");
        ui.horizontal_wrapped(|ui| {
            for mode in [Mode::Adaptive, Mode::Unseen, Mode::Mistakes, Mode::Due] {
                ui.selectable_value(&mut self.mode, mode, mode.label());
            }
        });
        ui.add_space(8.0);
        egui::ComboBox::from_label("分野")
            .selected_text(self.category.map_or("すべて", |i| CATEGORY_NAMES[i]))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.category, None, "すべて");
                for (i, name) in CATEGORY_NAMES.iter().enumerate() { ui.selectable_value(&mut self.category, Some(i), *name); }
            });
        ui.add_space(8.0);
        if ui.button(RichText::new("20問の演習を開始").size(17.0).strong()).clicked() { self.start_session(); }
        ui.add_space(16.0);
        ui.label("復習スケジュール: 正解で 1→3→7→14→30日と間隔を延長し、不正解なら翌日に再出題します。");
        ui.add_space(8.0);
        ui.colored_label(Color32::LIGHT_YELLOW, "注意: 収録32問はオリジナル学習問題です。法改正・出題年度への適合性は未検証です。");
    }

    fn study(&mut self, ui: &mut egui::Ui) {
        ui.heading("問題演習");
        if self.cursor >= self.session.len() { ui.label("ダッシュボードから演習を開始してください。"); return; }
        let q = self.questions[self.session[self.cursor]].clone();
        ui.label(format!("問題 {} / {}   ・   {}   ・   {}", self.cursor + 1, self.session.len(), category_name(&q.category), q.topic));
        ui.separator();
        ui.add_space(10.0);
        ui.label(RichText::new(&q.text).size(19.0).strong());
        ui.add_space(12.0);
        for (i, option) in q.options.iter().enumerate() {
            let is_correct = self.selected.is_some() && i == q.answer;
            let is_wrong = self.selected == Some(i) && i != q.answer;
            let text = RichText::new(format!("{}.  {}", i + 1, option)).size(16.0);
            let text = if is_correct { text.color(Color32::LIGHT_GREEN) } else if is_wrong { text.color(Color32::LIGHT_RED) } else { text };
            if ui.add_enabled(self.selected.is_none(), egui::Button::new(text).min_size(egui::vec2(ui.available_width().min(780.0), 42.0))).clicked() {
                self.submit(i);
            }
            ui.add_space(5.0);
        }
        if self.selected.is_some() {
            ui.separator();
            ui.heading(&self.feedback);
            ui.label(format!("解説: {}", q.explain));
            ui.add_space(10.0);
            if ui.button("次の問題へ").clicked() { self.next(); }
        }
        ui.add_space(18.0);
        if ui.small_button("演習を中断").clicked() { self.page = Page::Dashboard; }
    }

    fn statistics(&mut self, ui: &mut egui::Ui) {
        ui.heading("分野別の習熟状況");
        ui.add_space(8.0);
        for (i, cat) in CATEGORY_KEYS.iter().enumerate() {
            let questions: Vec<_> = self.questions.iter().filter(|q| q.category == *cat).collect();
            let mut attempts = 0u32;
            let mut correct = 0u32;
            let mut covered = 0usize;
            for q in &questions {
                if let Some(record) = self.progress.records.get(&q.id) {
                    attempts += record.attempts;
                    correct += record.correct;
                    covered += 1;
                }
            }
            let fraction = if attempts > 0 { correct as f32 / attempts as f32 } else { 0.0 };
            ui.label(RichText::new(format!("{}  /  {} 問中 {} 問学習済み  /  {} 回解答", CATEGORY_NAMES[i], questions.len(), covered, attempts)).strong());
            ui.add(egui::ProgressBar::new(fraction).text(format!("正答率 {:.0}%", fraction * 100.0)).desired_width(380.0));
            ui.add_space(14.0);
        }
        ui.separator();
        ui.add(egui::Slider::new(&mut self.progress.goal_per_day, 1..=200).text("1日の目標解答数"));
        ui.add_space(10.0);
        if ui.button("学習履歴をJSONでエクスポート").clicked() {
            match save_export(&self.progress) {
                Ok(path) => self.notice = format!("保存しました: {}", path.display()),
                Err(error) => self.notice = format!("保存失敗: {error}"),
            }
        }
        ui.colored_label(Color32::GRAY, "エクスポート先: ホームディレクトリ（日時付きファイル名）。進捗の自動保存はアプリ終了時・定期的に実行されます。");
    }
}

impl eframe::App for TakkenApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(RichText::new("宅建MASTER").color(Color32::from_rgb(115, 183, 255)));
            ui.separator();
            if ui.selectable_label(self.page == Page::Dashboard, "ホーム").clicked() { self.page = Page::Dashboard; }
            if ui.selectable_label(self.page == Page::Study, "演習").clicked() { self.page = Page::Study; }
            if ui.selectable_label(self.page == Page::Statistics, "分析・設定").clicked() { self.page = Page::Statistics; }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(8.0);
            match self.page {
                Page::Dashboard => self.dashboard(ui),
                Page::Study => self.study(ui),
                Page::Statistics => self.statistics(ui),
            }
            if !self.notice.is_empty() {
                ui.add_space(12.0);
                ui.colored_label(Color32::LIGHT_BLUE, &self.notice);
            }
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.progress);
    }
}

fn metric(ui: &mut egui::Ui, title: &str, value: String) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(150.0);
        ui.label(title);
        ui.label(RichText::new(value).size(24.0).strong());
    });
}

fn category_name(key: &str) -> &'static str {
    CATEGORY_KEYS.iter().position(|c| *c == key).map_or("その他", |i| CATEGORY_NAMES[i])
}

fn save_export(progress: &Progress) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from).unwrap_or(std::env::current_dir()?);
    let name = format!("takken-progress-{}.json", Local::now().format("%Y%m%d-%H%M%S"));
    let path = home.join(name);
    fs::write(&path, serde_json::to_vec_pretty(progress)?)?;
    Ok(path)
}

fn install_japanese_font(ctx: &egui::Context) {
    // Use an installed Japanese font: distributing proprietary OS fonts is not permitted.
    let candidates = [
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\YuGothM.ttc",
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
    ];
    for path in candidates {
        let Ok(bytes) = fs::read(path) else { continue; };
        let mut defs = FontDefinitions::default();
        defs.font_data.insert("japanese".into(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            defs.families.get_mut(&family).expect("default family").insert(0, "japanese".into());
        }
        ctx.set_fonts(defs);
        return;
    }
    eprintln!("Japanese font unavailable; install a Japanese font or add its path to install_japanese_font");
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1050.0, 750.0]).with_min_inner_size([700.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native("宅建MASTER", options, Box::new(|cc| Ok(Box::new(TakkenApp::new(cc)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_questions_have_valid_answers_and_unique_ids() {
        let qs: Vec<Question> = serde_json::from_str(include_str!("../data/questions.json")).unwrap();
        assert_eq!(qs.len(), 32);
        let mut ids = std::collections::BTreeSet::new();
        for q in qs { assert!(q.answer < 4); assert!(ids.insert(q.id)); assert!(!q.explain.is_empty()); }
    }
    #[test]
    fn correct_answers_extend_review_interval() {
        let mut state = Progress::default();
        let day = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        state.answer(1, true, day);
        assert_eq!(state.records[&1].next_due, Some(day + Duration::days(1)));
        state.answer(1, true, day);
        assert_eq!(state.records[&1].next_due, Some(day + Duration::days(3)));
        state.answer(1, false, day);
        assert_eq!(state.records[&1].next_due, Some(day + Duration::days(1)));
        assert_eq!(state.records[&1].repetitions, 0);
    }
}