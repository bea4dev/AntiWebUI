# 19. ボタンの部品

[15](15-md3e.md) のボタンに、アイコン、トグル、アイコンボタン、FAB、ボタングループを足す。値は同じく Jetpack Compose の Material 3（`androidx-main`、2026-11-04 時点）から取る。

```rust
use anti_webui_md3e::components::{button, styled_button, toggle_button, ...}; // 名前を挙げる（15 を参照）
use anti_webui_md3e::icons;

// アイコン付きのボタン
styled_button("保存", ButtonStyle::filled().icon(icons::SAVE))

// トグル。選択状態はシグナルなどから読み、変更はアプリが行う
toggle_button("太字", move || bold.get()).on_click(move |_| bold.update(|b| *b = !*b))
styled_toggle_button("通知", move || on.get(), ButtonStyle::tonal().icon(icons::NOTIFICATIONS))

// アイコンボタン
icon_button(icons::SEARCH)
filled_icon_button(icons::ADD)
tonal_icon_button(icons::EDIT)
outlined_icon_button(icons::DELETE)
styled_icon_button(icons::MENU, IconButtonStyle::filled().size(ButtonSize::Medium).width(IconButtonWidth::Wide))
icon_toggle_button(icons::FAVORITE, move || liked.get(), IconButtonStyle::standard())

// FAB
fab(icons::ADD)
styled_fab(icons::ADD, FabStyle::new().size(FabSize::Large).color(FabColor::Tertiary))
extended_fab(icons::ADD, "作成")
styled_extended_fab(icons::ADD, "作成", FabStyle::new().expanded_when(move || !scrolled.get()))

// ボタングループ
button_group([button("前へ"), tonal_button("再生"), button("次へ")])
connected_button_group(
    ["日", "週", "月"],
    ButtonStyle::filled(),
    move |i| tab.get() == i,
    move |i| tab.set(i),
)
```

どの部品も普通の `Div` を返す。

## 共通の部分

状態レイヤー、リップル、フォーカス、無効、影は `components/surface.rs` にまとめ、全部品で同じものを使う。[15](15-md3e.md) のとおり公開 API だけで書く。コアの変更はない。

- 押すと形が変わる部品は、4 つの角の半径を別々に `animated` で動かす。連結グループの内側の角だけが変わるように。止まっている間はフレームを要求しない。
- 枠線のある部品は、選択しても枠線の幅を残して色だけ透明にする。幅を消すと中身が動くため。Compose の枠線はレイアウトに影響しないので、見た目は同じになる。
- 押している間の影は通常の高さに戻す（Compose の押下時のトークンと同じ）。

## ボタンのアイコン

`ButtonStyle::icon` と `trailing_icon`。

| サイズ | アイコン | ラベルとの間 |
|---|---|---|
| ExtraSmall | 20 | 4 |
| Small | 20 | 8 |
| Medium | 24 | 8 |
| Large | 32 | 12 |
| ExtraLarge | 40 | 16 |

ExtraSmall の間は、トークンでは 8 だが Compose のコードが 4 を使っているので 4 にした。

## トグル

| 種類 | 通常の背景 / 内容 | 選択時の背景 / 内容 |
|---|---|---|
| filled | surfaceContainer / onSurfaceVariant | primary / onPrimary |
| tonal | secondaryContainer / onSecondaryContainer | secondary / onSecondary |
| elevated | surfaceContainerLow / primary | primary / onPrimary |
| outlined | なし / onSurfaceVariant + outlineVariant の枠線 | inverseSurface / inverseOnSurface |
| text | なし / primary | secondaryContainer / onSecondaryContainer |

text は Compose にないので、ここで決めた。

- 形は 丸 → 押下 → 選択（`square` の角丸）の順に優先して変わる。`square()` を付けると、通常が角丸、選択が丸になる。
- 形の変化は速い移動のばね（800、0.6）で、少し弾む。普通のボタンと違うのは Compose と同じ。
- 色は瞬時に切り替わる（Compose と同じ）。
- 付いているアイコンは、選択すると `FILL` が 1 になる（200 ms）。Compose は呼び出し側に任せているが、ここでは軸で動かせるので部品がやる。

## アイコンボタン

| サイズ | 高さ | アイコン | 左右の余白（狭い / 標準 / 広い） | 角丸 | 押下時 | 枠線 |
|---|---|---|---|---|---|---|
| ExtraSmall | 32 | 20 | 4 / 6 / 10 | 12 | 8 | 1 |
| Small | 40 | 24 | 4 / 8 / 14 | 12 | 8 | 1 |
| Medium | 56 | 24 | 12 / 16 / 24 | 16 | 12 | 1 |
| Large | 96 | 32 | 16 / 32 / 48 | 28 | 16 | 2 |
| ExtraLarge | 136 | 40 | 32 / 48 / 72 | 28 | 16 | 3 |

| 種類 | 背景 / 内容 | トグルの通常 | トグルの選択 |
|---|---|---|---|
| standard | なし / onSurfaceVariant | 同じ | なし / primary |
| filled | primary / onPrimary | surfaceContainer / onSurfaceVariant | primary / onPrimary |
| tonal | secondaryContainer / onSecondaryContainer | 同じ | secondary / onSecondary |
| outlined | なし / onSurfaceVariant + 枠線 | 同じ | inverseSurface / inverseOnSurface |

- 無効の内容は onSurface 38%、背景があれば onSurface 10%。
- 丸の半径は、幅と高さの短いほうの半分。
- トグルの形の変化は標準の効果（1600、1.0）で弾まない。Compose のコメントのとおり意図的なもの。

## FAB

| サイズ | 大きさ | アイコン | 角丸 | 拡張のアイコン | 拡張の左右 / 間 | 拡張の文字 |
|---|---|---|---|---|---|---|
| Baseline | 56 | 24 | 16 | 24 | 16 / 8 | Title Medium |
| Medium | 80 | 28 | 20 | 28 | 26 / 16 | Title Large |
| Large | 96 | 36 | 28 | 32 | 28 / 20 | Headline Small |

Large のアイコンは Compose のコードの値（36）で、トークンの 32 は「誤り」とコメントされている。拡張のほうは 32 のまま使っている。

- 色は `PrimaryContainer`（既定）、`SecondaryContainer`、`TertiaryContainer`、`Primary`、`Secondary`、`Tertiary`。
- 影は通常 Level 3、ホバー Level 4。Compose の押下とフォーカスは Level 3。
- 押しても形は変わらない。

**拡張 FAB の開閉。** Compose は幅を最小幅と本来の幅の間で補間する（速い移動のばね）。ラベルの透明度は速い効果のばね。ここでは、ラベルを `overflow_hidden` の箱に入れて、幅を `NodeRef::rect` で読んだラベルの幅 × 進み具合にする。開き切ると幅の指定を外すので、ラベルが変わっても追従する。閉じているときはアイコンを中央に置く。Compose は左に寄せるので、Large だけ 4 px 違う（Large は左右の余白を足しても 96 に届かない）。

## ボタングループ

### 普通のグループ

間は 12。押すと幅の 15% だけ広がり、隣が同じだけ狭くなる（速い移動のばね）。

| 位置 | 広がる量 | 隣から取る量 |
|---|---|---|
| 両端 | min(幅 × 0.15, 24) | 隣 1 つから全部 |
| 中間 | min(幅 × 0.075, 24) × 2 | 両隣から半分ずつ |

**Compose との違い。** Compose はグループが子の幅を直接決める。こちらの部品は `Div` なので、左右の余白を増減して幅を変える。

- 幅は内容から決まるので、計測の仕組みがいらない。内容は真ん中に残る。
- ボタンの境目が動く量を物理 px の整数で決め、余白はそこから割り振る。余白ごとに丸めると合計が 1 px ずつ増減して周りが揺れるので、こうしている。これで、あるボタンが増えた分は隣が減った分と常に等しく、グループの両端は動かない。両側に隣があるボタンは境目が同じだけ離れるので、押してもラベルが動かない。
- 幅を読むために各項目を 1 つの箱で包む（`NodeRef` は 1 つの要素に 1 つなので、利用者の指定を奪わない）。

### 連結グループ

間は 2。`ButtonStyle` の種類で全部のトグルを作る。

| 角 | 通常 | 押下 | 選択 |
|---|---|---|---|
| 外側 | 丸 | 丸 | 丸 |
| 内側 | 8 | 4 | 丸 |

トークンは Small の分しか公開されていないので、どのサイズでもこの値を使う。押しても幅は変わらない（Compose のサンプルのとおり）。

## 確認

`anti-webui-md3e/tests/buttons.rs`: アイコンの大きさと間、トグルの色と形、最初から選択されたトグル、枠線の幅、`FILL`、アイコンボタンの全サイズと全幅、FAB のサイズと色と影、拡張 FAB の開閉、グループの広がり方と上限、キーボード、連結グループの角、止まっている間はフレームを要求しないこと。

## 性能

押している間のフレーム（`bench_md3e --compile 1` と `--compile 2`、オフスクリーン、RTX 5070、1620×1296、倍率 1.8。ボタンは 2 回、グループは 3 回）。

| 押すもの | UI の p50 | UI の最大 | 描画の p50 | 描画の最大 |
|---|---|---|---|---|
| ボタン 30 個のページのボタン | 0.04〜0.05 ms | 0.14 ms | 2.2 ms | 3.7〜4.0 ms |
| 5 個のグループ 12 組のページのボタン（レイアウトが毎フレーム走る） | 0.08〜0.10 ms | 0.22〜0.26 ms | 2.3〜2.5 ms | 4.0〜4.7 ms |

描画には読み戻しを含む。グループでも UI の処理は 1 フレーム（120 Hz で 8.3 ms）に比べて十分小さい。

**未計測:** 実ウィンドウの CPU と電力、Chrome との比較（同じ動きのページを作っていない）、内蔵 GPU をコンポジタの出力にした状態と 60 Hz。

## やっていないこと

- グループのあふれメニュー。
- 項目の幅を均等に広げるグループ（Compose の `weight`）。

FAB メニューとスプリットボタンは [20](20-md3e-components.md)。
