# 21. 標準の部品

`anti-webui-widgets` の部品を、デスクトップアプリに必要なところまで揃える。`prelude` から使える、MD3E に頼らない標準の見た目である。

```
cargo run --release -p anti-webui --example widgets [ページ名]
```

## 方針

- **見た目は落ち着いたものにする。** 配色は GitHub Primer を元にする。動きは 120ms 前後のトランジションだけで、ばねもシェーダーも使わない。
- **core の公開 API だけで作る。** 依存を増やさない。チェック、シェブロンなどの記号はフォントの文字に頼る（`✓ ▸ ▾ ×`）。アイコンのクレートを標準の部品に持ち込まないためである。
- **部品は `Div`、`TextInput`、または `Show` を返す。** 続けて `.w(...)` や `.on_click(...)` を書ける。
- **状態は `Signal` で受ける。** MD3E はクロージャで受けるが、標準の部品は最初からこの形なので揃える。
- **長いものは見えている行だけ作る。** リスト、ツリー、テーブルは行数に比例しない。

## CSS と同じに書ける

このプロジェクトはブラウザの代わりでもあるので、色や形の変え方は CSS と同じ考え方にする。

| CSS | AntiWebUI | 違い |
|---|---|---|
| ブラウザの既定のスタイルシート | 部品の `Sheet` | 同じ。常に一番下の層 |
| `style="background: red"` | `button("x").bg(red)` | 同じ。ホバー中も赤のまま |
| `:hover`、`:active`、`:focus` | `.hover(...)`、`.active(...)`、`.focused(...)` | 同じ |
| `:focus-visible` | `Sheet::focus_visible` | `Sheet` の中でだけ書ける |
| `:disabled` | `Sheet::disabled` | 同じ。祖先の無効も受け継ぐ（`fieldset` と同じ） |
| `accent-color` | `.accent_color(c)` | 同じ。継承する |
| `transition` | `.transition(...)`、`.transition_of(...)` | 同じ |
| `<input>` のキャレットや選択色 | `.caret_color(c)`、`.selection_color(c)` | 同じ。キャレットは指定がなければ文字の色になる（`caret-color: auto`） |
| `scrollbar-color` | `.scrollbar_color(c)` | つまみの色だけ |
| `appearance: none` と独自の見た目 | `div()` から組む | 同じ |
| セレクタ、クラス | なし | M2 の CSS サブセット |

### 重なり順

```
継承 → Sheet（基本 → hover → focus → focus_visible → active → disabled → accent）
     → 要素の基本 → dyn_style → .hover → .focused → .active
```

- `Sheet` のすべてが要素自身の指定より下にある。ブラウザのスタイルシートがページのスタイルより下にあるのと同じ。
- 部品の根の要素は、見た目の箱そのものにする。`select(...).bg(c)` や `number_field(...).border_color(c)` は見た目の箱に効く。
- 部品は根の色を `dyn_style` で書かない。`dyn_style` は要素の基本より上なので、利用者の `.bg(c)` に勝ってしまう。状態による違いは `Sheet` を差し替えて表す。

### `Sheet`

```rust
pub fn button(label: impl IntoTextSource) -> Div {
    div().px(12.0).sheet(|| shared(button_sheet)).child(text(label))
}

fn button_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.surface_muted).border_color(t.border))
        .hover(|s| s.bg(t.hover))
        .disabled(|s| s.text_color(t.text_disabled))
}
```

`shared` はテーマごとに `Sheet` を 1 つだけ作る。同じ種類の部品は同じ `Rc<Sheet>` を指すので、状態ごとのスタイルを部品の数だけ持たない。読むと `theme()` を追跡するので、テーマを変えると各ノードのエフェクトが新しい `Rc` に差し替える。

## テーマ

```rust
set_theme(Theme::dark());
set_theme(Theme { accent: rgb(0x8250df), ..Theme::light() });
let t = theme(); // 読むと追跡する
```

- `Theme` はただの色の集まりで、アプリが自由に作れる。
- `page()` が根の要素である。背景、文字色、キャレット、選択色、スクロールバーの色を設定し、下へ継承させる。ポップアップとポータルも宣言された場所から継承する。

## 部品

| 分類 | 部品 |
|---|---|
| 土台 | `page`、`Theme`、`theme`、`set_theme`、`scroll_view`、`card`、`heading`、`label`、`link`、`divider` |
| ボタン | `button`、`primary_button`、`danger_button`、`quiet_button`、`toggle_button` |
| 選択 | `checkbox`、`tri_checkbox`（`CheckState`）、`radio`、`radio_group`、`switch`、`slider`、`stepped_slider`、`select`、`list_box`、`multi_list_box` |
| 文字入力 | `text_field`、`text_area_field`、`number_field`、`search_field` |
| 表示 | `badge`、`toned_badge`（`Tone`）、`progress_bar`、`indeterminate_progress_bar`、`spinner`、`sized_spinner`、`tooltip` |
| 通知 | `toast`、`toast_with`（`Toast`）、`toast_host` |
| 構造 | `tabs`、`tab`、`disclosure`、`accordion`、`dialog`、`alert_dialog`、`toolbar`、`toolbar_separator`、`toolbar_spacer` |
| メニュー | `menu`、`dropdown_button`、`context_menu`、`menu_item`、`menu_separator`、`submenu` |
| 長いもの | `virtual_list`、`scroll_row_into_view`、`tree_view`、`tree_item`、`table`、`column`、`custom_column` |
| 配置 | `split_row`、`split_col`、`breadcrumb`、`crumb`、`pagination` |

### キーボード

| 部品 | キー |
|---|---|
| ボタン、チェックボックス、スイッチ | Tab で移動、Enter と Space で押す |
| `radio_group`、`tabs` | 全体で Tab 1 回。矢印で移動して選ぶ |
| `slider` | 矢印、PageUp / PageDown、Home / End |
| `number_field` | 上下で増減。Enter かフォーカスを外すと確定する。読めない値は元に戻し、範囲の外は範囲に収める。増減はステップの数から計算するので誤差が積もらない。ステップの間の値は隠さずに見せ、増減すると次のステップへ行く |
| `search_field` | Escape で消す |
| `select`、`list_box` | 上下、Home / End、`select` は Enter / Space で決定 |
| `multi_list_box` | 上下、Space で切り替え、Shift+上下で範囲。Ctrl+クリック、Shift+クリック |
| `menu` | 上下、Home / End、Enter / Space で実行、右でサブメニュー、左で戻る、Escape で閉じる、Tab で全部閉じる |
| `dialog` | Tab は中で回る、Escape で閉じる、閉じるとフォーカスが戻る |
| `tree_view` | 上下、左で閉じるか親へ、右で開くか子へ、Home / End、Enter / Space で開閉 |
| `table` | 上下、PageUp / PageDown、Home / End |
| `split_row`、`split_col` | 分割線で矢印 |
| `sortable_list`、`sortable_row` | Alt と矢印で項目を動かす |

メニュー、ダイアログ、`select` のリストの中で押したものとキーは、宣言された場所の祖先に伝えない。祖先はそこで起きたことを知らないからである（たとえば `dropdown_button` は押されると開閉する）。

トーストはポインタが乗っている間は待つ。

## 見えている行だけ作る

```rust
virtual_list(move || items.with(Vec::len), 28.0, move |i| text(format!("row {i}")))
```

- 行の高さは一定で、論理ピクセルで渡す。
- 行の位置は `Length::Device(i × 行の物理高さ)` にする。論理ピクセルの丸めを積み上げないので、拡大率 1.8 でも隙間もずれも出ない。
- スクロールのたびに範囲を計算する。範囲の先頭は 4 行単位に丸めてあり、上下に 8 行余分に作る。
- 物理高さの合計はおよそ 1600 万ピクセルまで滑らかに動く（`f32` の精度。28px の行で約 30 万行）。Chromium にも同じような上限がある（約 3300 万ピクセル）。

`tree_view` と `table` はこの仕組みで動く。データや並びが変わると、見えている行を作り直す。

## 測ったもの

`cargo run --release -p anti-webui --example widgets -- --bench`。2340×1616、拡大率 1.8、ウィンドウなしの UI スレッドの時間（ミリ秒）。各ページの 2 周目。

| ページ | 作る | 最初のフレーム | 以後の中央値 | 以後の最大 |
|---|---|---|---|---|
| buttons | 0.10 | 0.49 | 0.01 | 0.02 |
| selection | 0.12 | 0.97 | 0.01 | 0.02 |
| inputs | 0.10 | 0.47 | 0.01 | 0.01 |
| feedback | 0.07 | 0.34 | 0.05 | 0.06 |
| structure | 0.09 | 0.61 | 0.01 | 0.01 |
| menus | 0.05 | 0.17 | 0.00 | 0.01 |
| data | 2.89 | 1.73 | 0.00 | 0.03 |
| layout | 0.89 | 0.49 | 0.01 | 0.01 |

10 万行のテーブルを 1 フレームあたり 40 物理ピクセルずつスクロールしたとき、入力の処理を含めて中央値 0.16、99 パーセンタイル 0.20〜0.22、最大 0.27〜0.64（2 回の計測）。`data` の「作る」には、サンプルが 10 万行のデータを作る時間と並びの計算が含まれる（内訳は測っていない）。

GPU の時間、実ウィンドウでの CPU と消費電力、Chrome との比較は測っていない。

## core に足したもの

| 追加 | 内容 |
|---|---|
| `Sheet`、`.sheet(...)` | 上で説明した層 |
| `:focus-visible` | `WindowState::focus_visible` |
| 無効の継承 | `ComputedStyle::disabled`。祖先が無効なら、中の要素も押せず、フォーカスも移らない。無効になった要素はフォーカスを失う |
| `accent_color` | 継承する色と、`Sheet` の `accent_background` などの印 |
| `scrollbar_color` | 固定だった色を継承するスタイルに |
| `Length::Device` | 物理ピクセルをそのまま指定する |
| `window_scale()` | 窓の拡大率。読むと追跡する |

## MD3E との関係

重なる処理（ツールチップの待ち時間、モーダルの枠組み）は共通化しない。必要な量が少なく、動きが違うからである。MD3E が必要とする共通部分は `portal`、`modal`、`set_timeout` としてすでに core にある。

画像を持つ部品（`image_view`、`avatar`、`image_avatar`、`media_card`）は [22](22-images.md)、並べ替え（`sortable_list`、`sortable_row`）は [23](23-drag-and-drop.md) にある。

## 今回やらないこと

- 文字を打って項目へ飛ぶ操作（`select`、`menu`、`list_box`）
- メニューバー（CSD と一緒に考える）
- `table` の複数行選択と、セルの編集
- `tree_view` の遅延読み込み
- 行の高さが一定でない `virtual_list`
- 検索欄の虫眼鏡アイコン（記号の字形がフォントに左右されるため）
- `:focus-visible` と `:disabled` を `Sheet` の外で書くこと
- テーマの切り替えを `page` ごと滑らかにすること（ボタンなど `smooth` の付いた部品だけが滑らかに変わる）

## 確認していないこと

- 実機でのポインタとキーボードの操作。テストはすべて合成イベントである
- 記号（`✓ ▸ ▾ × ▲ ▼ ‹ ›`）の字形が、フォントの少ない環境でどう見えるか
- 内蔵 GPU をコンポジタの出力にした状態と 60 Hz
