# 13. アニメーション API

`request_frame` だけで書くと、途中からの切り返し、要素が消えたときの停止、イージングを毎回自分で書くことになる。これらをフレームワークが引き受ける API を 2 つ用意する。

| | 向いている場面 | 動かすもの |
|---|---|---|
| `transition` | スタイルが変わるときに、なめらかにつなぎたい | フレームワークが途中の値を計算する |
| `animated` | クリックやドラッグの結果など、値を自分で動かしたい | シグナルのように読める値 |

## `transition`

```rust
div()
    .bg(WHITE)
    .hover(|s| s.bg(BLUE).scale(1.05))
    .transition(Transition::ms(150).ease_out())

// 対象を絞る
div().transition_of([Prop::Scale, Prop::Opacity], Transition::ms(200).ease_in_out())
```

- `hover`、`focused`、`active`、`dyn_style`、どれで値が変わっても動く。
- CSS と同じく、**変更後のスタイル**に書かれた `transition` を使う。`hover(|s| s.scale(1.1).transition(速い))` と書くと、入るときだけ速くできる。
- 継承しない。

### 対象

| `Prop` | プロパティ | コスト |
|---|---|---|
| `Translate`, `Scale`, `Rotate`, `Opacity`（変形のあるノード） | 変形と不透明度 | 合成だけ（[12](12-transform.md)） |
| `Opacity`（変形のないノード）, `Background`, `BorderColor`, `OutlineColor`, `TextColor`, `Radius`, `Shadow` | 色、角丸、影 | 描き直し |
| `Width`, `Height`, `Margin`, `Padding`, `Inset`, `Gap`, `BorderWidth` | 大きさと位置 | レイアウト |

`Length` は `Px` 同士、`Percent` 同士だけつなぐ。`Auto` が絡むとすぐ切り替わる（CSS と同じ）。影は同じ順の同士をつなぎ、数が違うときは足りない分を透明な同じ影とみなす。内側と外側が違うとすぐ切り替わる。

色はプリマルチプライドで補間する。`TRANSPARENT` から青へ変えても途中で黒ずまない。

### 動かない場合

- 作られたばかりのノード、`display: none` だったノード、`display: none` の中のノード。変化の前の見た目が無いので、最終の値から始まる。
- `Transition::ms(0)`、または対象に入っていないプロパティ。

### 途中で変わったとき

- 今の値から、新しい目標へつなぐ。
- 元の値へ戻るとき（ホバーして、すぐ離れたとき）は、CSS の reversing shortening と同じく、それまで進んだ割合だけ時間を縮める。少し動いただけなのに戻りに全部の時間が掛かると、のろく感じるため。
- 新しいスタイルに `transition` が無ければ、すぐ最終の値になる。

### 実装（core）

`resolve_node` で、スタイルの層を重ねた結果（目標）を計算した後に次を行う。

1. 動いているプロパティ: 目標が変わっていれば、つなぎ直す。変わらなければ、今の値で上書きする。
2. 動いていないプロパティ: 前回の結果と違い、`transition` があれば、動かし始める。
3. 終わったものは、目標の値にして取り除く（ここで次のアニメーションを始めないよう、終わる印を付けて `resolve_node` で取り除く）。

`Ui::animate` は動いているノードの今の値を更新し、`mark_style_of` を呼ぶだけで、あとは通常の経路（スタイル → レイアウト → ペイント）を通る。これにより:

- 変形だけが動くなら、[12](12-transform.md) の経路で内容を描き直さない。
- 色の継承、レイアウトの無効化、差分がそのまま効く。
- ノードが消えれば、動いていた情報も一緒に消える（`Node` が持つ）。

`dyn_style` は目標が変わったときに 1 回呼ばれるだけで、毎フレームは呼ばれない。

開始時刻は `resolve_node` を通った時の `Instant::now()`。次のフレームの `now` との差から始まるので、最初の 1 フレームを無駄にしない。

## `animated`

```rust
let x = animated(0.0_f32);

div()
    .dyn_style(move |s| s.translate_x(x.get()))
    .on_click(move |_| x.to(300.0, Transition::ms(300).ease_out()));

x.spring_to(300.0, Spring::default());
x.set(0.0);   // すぐ変える
x.stop();     // 今の値で止める
```

- `Signal` と同じく `Copy` で、`get` は依存関係を登録する。`dyn_style` は動いている間だけ毎フレーム呼ばれる。
- `T` は `f32`、`(f32, f32)`、`Color`（`Animatable` を実装すれば増やせる）。
- 所有者が破棄されると止まる（`request_frame` を手で使うときのような panic は起きない）。
- 動いている途中で `to` や `spring_to` を呼ぶと、今の値からつなぐ。`spring_to` は速度も引き継ぐ。

### Spring

```rust
Spring { stiffness: 170.0, damping: 26.0, mass: 1.0 }   // 既定（react-spring の default 相当）
```

時間ではなくバネの性質で指定する。途中で目標が変わっても動きが途切れない。

- 1 ms ごとの固定ステップで積分する。フレームの間隔に関わらず同じ動きになる。
- 動いた距離の 0.05% 以内に入り、速度も小さくなったら止まる。

### フレームごとの処理

`request_frame` のコールバックはまとめて `batch` の中で実行する。複数の `animated` を読む `dyn_style` でも 1 フレームに 1 回しか呼ばれない。

## リアクティブの計算量

アニメーションでは多くの effect が毎フレーム同時に動くので、2 つ直した。

- `flush` は次の effect を探すのに待ち行列全体を見ていた。深さと順番のヒープにした。
- 再実行のたびに依存を全部外して登録し直し、シグナル側の配列も毎回探していた。前回と同じ順に読んでいる間は何もしないようにした。読まなくなったものだけを最後に外す。

| 同時に動く effect | 修正前 | 修正後 |
|---|---|---|
| 10 | 約 1 µs | 0.5 µs |
| 100 | 約 50 µs | 5.6 µs |
| 1000 | 約 4.5〜5.1 ms | 約 64 µs |
| 5000 | 約 112 ms | 約 0.35 ms |

## 計測

同時に動く数ごとの CPU の時間（1 フレーム、animate + update + paint、1620×1616、リリースビルド、ms）。

| 動かし方 | 100 個 | 1000 個 |
|---|---|---|
| `transition`（不透明度） | 0.04〜0.06 | 0.40〜0.47 |
| `transition`（`translate`） | 0.06〜0.08 | 0.72〜0.78 |
| 要素ごとの `animated` | 0.04 | 0.43〜0.49 |
| 1 つの `animated` を全要素で読む | 0.05〜0.08 | 0.59〜0.66 |

1000 個が同時に動いても 120 Hz の 1 フレーム（8.33 ms）の 1 割に届かない。GPU の時間は含まない。

実機（ShojiWM、120 Hz、RTX 5070）で、`transition`、`animated`、`Spring` のカードを同時に繰り返し動かすと、4 秒で 471〜475 フレームだった。

## やっていないこと

- 要素が消えるときのアニメーション（`show(...).enter(...).exit(...)`）。次の段階。
- キーフレーム、ループ、遅延。`animated` と `request_frame` で書ける。
- `transition` での `Spring`。
- `prefers-reduced-motion` に当たる設定。
