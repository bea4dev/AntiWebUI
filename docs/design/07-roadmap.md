# 07. ロードマップ

## M1: Wayland 上の基本 UI

- [x] リアクティブシステム（signal / memo / effect / batch / owner）
- [x] ノードツリー・ビルダー API・動的な子（`dyn_view` / `show` / `for_each`）
- [x] スタイル（型付き API、hover / active / focus パッチ、継承）
- [x] 物理整数ピクセルのスタイル解決と自前 Flex レイアウト
- [x] wgpu レンダラ（角丸矩形・枠線・影・グリフ）
- [x] Wayland バックエンド（fractional scale、入力、カーソル形状）
- [x] ドキュメント横断のテキスト選択（ドラッグ、ダブル/トリプルクリック、コピー、primary selection）
- [x] TextInput / TextArea（編集、クリップボード、undo）
- [x] IME（text-input-v3、preedit 表示）
- [x] 基本ウィジェット: Button, Checkbox, Switch, Slider, ScrollView, TextField, Card
- [x] オフスクリーン描画（`headless`）と、合成イベントによる統合テスト
- [x] ダメージトラッキング（変更矩形の計算・部分描画・Wayland の dmabuf presenter と `damage_buffer`）
- [x] スクロールレイヤー（上下左右のスクロールで、動いた分だけ描く）

### M1 の既知の制限

- 実機での確認が必要: IME（fcitx5 等）・クリップボード・primary selection の Wayland 往復
- 選択ドラッグがウィンドウ端に達したときの自動スクロールは未実装
- パスワード入力のマスク表示は未実装（`ImePurpose::Password` は IME へのヒントのみ）
- スタイル解決とペイントは毎回全ノードを走査する（レイアウトは差分化済み。[03](03-layout.md) 参照）
- `opacity` は子孫の色に乗算する近似（グループ不透明度ではない）
- サーバ側装飾が無いコンポジタではタイトルバーが出ない（CSD は M2）

## M2: 実用性

- CSS サブセット（スタイルシート・クラス・ホットリロード）
- `flex-wrap`、Grid
- 画像（PNG/JPEG/SVG）
- CSD（クライアント側装飾）、ポップアップ（xdg_popup）、複数ウィンドウ
- AccessKit によるアクセシビリティ
- 要素インスペクタ

## M3: 多言語・多プラットフォーム

- `anti-webui-ffi`（C ABI）、C++ / C# バインディング
- Windows / macOS / X11 バックエンド（それぞれの presenter でダメージトラッキング。[05](05-rendering.md) 参照）

## M4: おまけ機能

- 動画再生（外部テクスチャ）、WebRTC、画面キャプチャ、通知
