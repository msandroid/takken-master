# 宅建MASTER — Rust + egui Desktop

宅地建物取引士の試験学習に向けたネイティブGUIアプリ。既存の `index.html` ブラウザ版とは別実装です。

## 実行

Rust 1.95以降（eframe 0.37の最低対応バージョン）を用意して以下を実行します。

```bash
cargo run --release
```

Windows 11、macOS、Linuxのデスクトップを対象としています。日本語の表示には、Windowsのメイリオ・游ゴシック、macOSのヒラギノ、LinuxのNoto CJKのいずれかがインストールされている必要があります。

## 機能

- 4択演習と回答直後の解説
- 32問のオリジナル問題（`data/questions.json`、法改正検証前の教材）
- 弱点・未解答・復習期限到来・適応型出題
- 間隔反復（正解 1→3→7→14→30日、不正解は翌日に復習）
- 分野別正答率、直近30問、日次目標
- `eframe` のネイティブ永続化、学習履歴JSONエクスポート

## テスト

```bash
cargo test
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

## 注意

本アプリの収録問題は過去問ではなく独自作成問題です。公式試験や最新法令への適合性を保証しません。初期リリースではWindows/macOSのGUI操作テストおよびCIビルドが未検証です。問題を解く際は受験年度の法令と公式情報を確認してください。