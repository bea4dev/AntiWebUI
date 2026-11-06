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
- [x] 基本ウィジェット: Button, Checkbox, Switch, Slider, ScrollView, TextField, Card（[21](21-widgets.md) で一式に）
- [x] オフスクリーン描画（`headless`）と、合成イベントによる統合テスト
- [x] ダメージトラッキング（変更矩形の計算・部分描画・Wayland の dmabuf presenter と `damage_buffer`）
- [x] スクロールレイヤー（上下左右のスクロールで、動いた分だけ描く）
- [x] CPU 側の差分化（スタイル解決は変わったノードだけ、ペイントとヒットテストは見える範囲だけ）
- [x] 慣性スクロールと全体設定（[08](08-scrolling-and-settings.md)）
- [x] CSS と同じ影（外側・内側）、`z_index`、ポップアップ、プルダウンリスト（[09](09-shadows-stacking-popups.md)）
- [x] フレームの先行（Auto / Enable / Disable / Custom）、フレーム計測、`request_frame`（[10](10-frame-pacing.md)）
- [x] `translate` / `scale` / `rotate` と合成レイヤー（[12](12-transform.md)）
- [x] `transition`、`animated`、`Spring`、`show` と `for_each` の `enter` / `exit`、`for_each` の `moves`（[13](13-animation.md)）
- [x] グループ不透明度（[12](12-transform.md)）
- [x] カスタムシェーダーの `fill` / `overlay`（[14](14-shaders.md)）
- [x] アプリに同梱したフォントの登録（[04](04-text.md)）
- [x] Material 3 Expressive の部品の最小版（[15](15-md3e.md)）
- [x] `filter`、`blur`、`backdrop_blur`、入力を読むシェーダー（[16](16-filters.md)）
- [x] 可変フォントの軸の指定とアニメーション（[17](17-font-axes.md)）
- [x] Material Symbols のアイコン（[18](18-icons.md)）
- [x] MD3E のトグル、アイコンボタン、FAB、ボタングループ（[19](19-buttons.md)）
- [x] MD3E の残りの部品、`portal`、`set_timeout`、ショーケース（[20](20-md3e-components.md)）
- [x] 標準の部品の一式、`Sheet`、テーマ、見えている行だけ作るリストとテーブル（[21](21-widgets.md)）
- [x] つかみ、アプリ内と OS の DnD、並べ替え（[23](23-drag-and-drop.md)）
- [x] 選択した文字のドラッグとテキスト入力へのドロップ（[23](23-drag-and-drop.md)）
- [x] 動画（`video`、FFmpeg は実行時に読み込む）（[24](24-video.md)）
- [x] `flex-wrap`、`align-content`（[03](03-layout.md)）
- [x] パス描画、`svg` と形のノード、SVG ファイルの読み込み（[25](25-svg.md)）
- [x] ウィンドウと要素のフルスクリーン（[27](27-fullscreen.md)）
- [x] `rich_text`（段落の中の `span` とノード）（[28](28-markdown.md)）
- [x] `text_editor`（1 行を 1 つのノードにし、装飾で隠す、置き換える、スタイルを付ける）（[28](28-markdown.md) の M3）

### M1 の既知の制限

- 実機での確認が必要: IME（fcitx5 等）・クリップボード・primary selection の Wayland 往復
- フレームの先行は、コンポジタが AMD で動く構成で NVIDIA dGPU も有効なときを未確認
- パスワード入力のマスク表示は未実装（`ImePurpose::Password` は IME へのヒントのみ）
- サーバ側装飾が無いコンポジタではタイトルバーが出ない（CSD は M2）
- カスタムシェーダー、MD3E の部品、`filter` と backdrop、可変フォントの軸、アイコン、画像は、内蔵 GPU をコンポジタの出力にした状態と 60 Hz での性能が未計測

## M2: 実用性

- カスタム影（[14](14-shaders.md)）、ウィンドウの後ろのぼかし（`ext-background-effect-v1`）
- MD3E の入力モード（日付、時刻）、ロケール（[20](20-md3e-components.md)）
- CSS サブセット（スタイルシート・クラス・ホットリロード）
- Grid
- CSD（クライアント側装飾）、複数ウィンドウ
- AccessKit によるアクセシビリティ
- 要素インスペクタ

## M3: 多言語・多プラットフォーム

- `anti-webui-ffi`（C ABI）、C++ / C# バインディング
- Windows / macOS / X11 バックエンド（それぞれの presenter でダメージトラッキング。[05](05-rendering.md) 参照）

## M4: おまけ機能

- PDF ビュワー（`hayro` を土台に、PDF.js 以上の速度を目標。[26](26-pdf.md) 参照。P4、フォーム、注釈、プレゼンテーションモード、フォームの JavaScript（`pdf-js` feature）まで実装済み）
- Markdown のビュワーとエディタ（[28](28-markdown.md) 参照。ビュワーと Mermaid（`markdown` feature）、画面から遠いブロックを後回しにする `content-visibility`（[29](29-content-visibility.md)）、Obsidian のようなライブプレビューのエディタ `markdown_editor` まで実装済み。次は Markdown 用のコードエディタ（ソースモード））

- WebRTC、画面キャプチャ、通知
