# 34. 読み上げ（アクセシビリティ）の口

スクリーンリーダーなどの支援技術が要るものを core に置く。プラットフォームの読み上げの仕組み（Linux では AT-SPI）へ橋渡しすることはしない。橋渡しは別のクレートが公開 API でやる。

## 方針

- **口だけを core に置く。** 要るのは次の 3 つである。
  1. ノードが何で、何と呼ばれ、どんな状態か
  2. その木と、それが変わったこと
  3. 支援技術からの頼みごと（フォーカス、押す、値を増やす・減らす）
- **依存を増やさない。** AccessKit で AT-SPI につなぐと、zbus と非同期のランタイムが入る。使いたいアプリだけがつなげばよいので、標準のビルドには入れない。
- **見ている人がいなければ何もしない。** 木を作るのは、`snapshot` が呼ばれたときと、`watch` で見ている人がいるときだけである。
- **Chromium と同じ形にする。** Chromium も中に読み上げ用の木（アクセシビリティツリー）を持ち、AT-SPI へ渡すのは外側の層である。役割と状態の名前は ARIA に合わせる。

## 書き方

```rust
div()
    .focusable()
    .access(move |a| a.role(Role::Switch).label("Wi-Fi").checked(on.get()))
```

- `.access(f)` の `f` は、読んだシグナルが変わると呼び直される。
- 何度か書くと順に足され、後に書いたものが上に来る。標準の部品では、動きの層（`x_with`）が役割と状態を付ける。そのため見た目を丸ごと差し替えても（Lv4）、言うことは変わらない。

`Access` で言えること：

| 項目 | 中身 |
|---|---|
| `role` | ボタン、チェックボックス、スライダー、タブなど 40 余り（ARIA の役割） |
| `label`、`description` | 呼び名と説明 |
| `checked` | `False`、`True`、`Mixed` |
| `selected`、`expanded` | 選ばれているか、開いているか |
| `value`、`step`、`value_text` | 範囲の値と、言葉での値 |
| `level` | 見出しや木の行の深さ |
| `live` | 変わったときに告げるか（トーストは `Polite`） |
| `multiselectable` | 複数選べるリスト |
| `hidden` | 見た目だけのもの。中身ごと外す |

## 自分で決まるもの

- **呼び名：** ボタン、チェックボックス、タブ、メニュー項目、リストの選択肢などは、`label` がなければ中の文字から呼び名を作る（ARIA の name from content）。
- **文字：** 文字のノードは `Text` になる。
- **入力欄：** 打ち込む欄は `TextInput`（複数行は `MultilineTextInput`）になり、中身が `value_text` に入る。
- **画像：** `label` のある画像は `Image` になる。
- **core の状態：** 無効、フォーカスできるか、今のフォーカス、枠（論理ピクセル）は core が持っているものから入れる。
- **何も言わない箱：** 役割も呼び名もなく、フォーカスもできない箱は木から外し、中身を親に渡す。レイアウトのための箱が木に出てこないようにするためで、Chromium がそういうノードを無視するのと同じである。

## 橋渡しのための API（`anti_webui::prelude::accessibility`）

| 名前 | 中身 |
|---|---|
| `snapshot() -> AccessTree` | 今の木。最後の更新のレイアウトによる |
| `watch(f) -> AccessWatch` | 今の木ですぐに呼び、それからは木が変わった更新のあとに呼ぶ。UI のスレッドで呼ぶ。`stop()` で止める |
| `perform(id, AccessAction)` | 次の更新で行う。別のスレッドからは `ui_channel` で UI のスレッドへ送る |

- **`AccessTree`：** `root`、`focus`、`nodes`（文書の順）を持つ。
- **`AccessNode`：**
  - `id` はノードがある限り変わらない。
  - 役割、呼び名、状態、値、`bounds`、`children` を持つ。
- **`AccessAction`：**

  | 頼みごと | していること |
  |---|---|
  | `Focus` | フォーカスを移す |
  | `Click` | クリックと同じ |
  | `Increment`、`Decrement` | そのノードにフォーカスを移して、Up と Down のキーを送る |
  | `ScrollIntoView` | 見えるところまでスクロールする |

  `Increment` と `Decrement` をキーで送るのは、部品のキー操作をそのまま使うためである。スライダーが自分で増やし方を持たなくて済む。

### 木が変わったことを知る方法

- 文書の中で、木が変わりうることが起きたら、印を付ける。対象は次のとおり。
  - スタイル、レイアウト、木の形が変わった
  - フォーカスが移った
  - スクロールした
  - `.access` の中身が変わった
- 更新の終わりに、印があって見ている人がいれば木を作り直す。前と違ったときだけ伝える。
- 色だけのアニメーション（描き直しだけ）では木を作らない。

## 標準の部品と MD3E

役割は動きの層が付ける。

| 部品 | 役割と状態 |
|---|---|
| `button` | `Button` |
| `toggle_with`（トグルボタン、チップ） | `ToggleButton` と `checked` |
| `switch_with` | `Switch` と `checked` |
| `checkbox_with`、`tri_checkbox_with` | `Checkbox` と `checked`（`Mixed` も） |
| `radio_with`、`radio_group_with` | `Radio` と `checked`、`RadioGroup` |
| `slider_with`、`range_slider_with` | つまみ（つまみがなければ全体）が `Slider` と値・刻み |
| `tabs_with` | `TabList`、各タブが `Tab` と `selected`。標準の `tabs` の中身は `TabPanel` |
| `tooltip_with` | 中身が `Tooltip` |
| `modal_with` | `Dialog` |
| `menu_with` | `Menu`、項目が `MenuItem`、サブメニューは `expanded`。チェックのある項目は `MenuItemCheckbox` |
| `select_with` | `ComboBox` と `expanded`、開いたリストが `ListBox`、選択肢が `ListOption` |
| `list_box_with`、`multi_list_box_with` | `ListBox`（複数は `multiselectable`）、`ListOption` と `selected` |
| `tree_view_with` | `Tree`、行が `TreeItem` と深さ、`expanded`、`selected` |
| トースト、スナックバー | `Status`（告げ方は `Polite`） |
| `progress_bar`、MD3E の進捗 | `ProgressBar` と値 |

MD3E は、動きを借りている部品では同じ役割になる。そのほかに、次の部品に役割を付けた。

| 部品 | 役割 |
|---|---|
| `surface`（ボタン、FAB、クリックできるカード、チップ） | `Button` |
| ナビゲーションの項目 | `Button` と `selected` |
| 押せるリストの行 | `Button` |
| テキストフィールド | ラベルを呼び名にする |

## やっていないこと

- **AT-SPI への橋渡しのクレート。** AccessKit の `accesskit_unix` を使えば、`watch` の木を AccessKit の木へ移して、頼みごとを `perform` に渡すだけで作れるはずである。
- **文字の中の位置。** キャレットや選択範囲を支援技術へ渡すこと（AccessKit の text runs）はしていない。
- **値を直接決める頼みごと（`SetValue`）。** 入力欄やスライダーに直接値を入れる頼みは、まだない。
- **テーブル。** 行とセルの役割は付けていない。
- **MD3E の日付と時刻のピッカー、カルーセル。** 役割を付けていない。
