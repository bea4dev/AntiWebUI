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
- [x] CPU 側の差分化（スタイル解決は変わったノードだけ、ペイントとヒットテストは見える範囲だけ）
- [x] 慣性スクロールと全体設定（[08](08-scrolling-and-settings.md)）
- [x] CSS と同じ影（外側・内側）、`z_index`、ポップアップ、プルダウンリスト（[09](09-shadows-stacking-popups.md)）
- [x] フレームの先行（Auto / Enable / Disable / Custom）、フレーム計測、`request_frame`（[10](10-frame-pacing.md)）
- [x] `translate` / `scale` / `rotate` と合成レイヤー（[12](12-transform.md)）
- [x] `transition`、`animated`、`Spring`（[13](13-animation.md)）

### M1 の既知の制限

- 実機での確認が必要: IME（fcitx5 等）・クリップボード・primary selection の Wayland 往復
- フレームの先行は、コンポジタが AMD で動く構成で NVIDIA dGPU も有効なときを未確認
- 選択ドラッグがウィンドウ端に達したときの自動スクロールは未実装
- パスワード入力のマスク表示は未実装（`ImePurpose::Password` は IME へのヒントのみ）
- `opacity` は子孫の色に乗算する近似（グループ不透明度ではない）。`transform` のあるノードだけはグループ不透明度になる（[12](12-transform.md)）
- サーバ側装飾が無いコンポジタではタイトルバーが出ない（CSD は M2）

## M2: 実用性

- CSS サブセット（スタイルシート・クラス・ホットリロード）
- `flex-wrap`、Grid
- 画像（PNG/JPEG/SVG）
- CSD（クライアント側装飾）、複数ウィンドウ
- AccessKit によるアクセシビリティ
- 要素インスペクタ

## M3: 多言語・多プラットフォーム

- `anti-webui-ffi`（C ABI）、C++ / C# バインディング
- Windows / macOS / X11 バックエンド（それぞれの presenter でダメージトラッキング。[05](05-rendering.md) 参照）

## M4: おまけ機能

- 動画再生（外部テクスチャ）、WebRTC、画面キャプチャ、通知
