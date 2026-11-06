# 06. Wayland バックエンド

`wayland-client` + `wayland-protocols` を直接使う（ツールキットは使わない）。キーボードは `libxkbcommon`。

## 使用プロトコル

| プロトコル | 用途 | 必須 |
|---|---|---|
| `wl_compositor`, `xdg_wm_base` | ウィンドウ、`xdg_popup` | ○ |
| `wl_subcompositor` | バブル（[09](09-shadows-stacking-popups.md)） | 任意（無ければ grab なしの `xdg_popup`） |
| `wp_fractional_scale_v1` + `wp_viewporter` | 小数スケール | 推奨（無ければ `preferred_buffer_scale` の整数スケール） |
| `zxdg_decoration_manager_v1` | サーバ側装飾 | 推奨（無い場合は装飾なし。CSD は将来） |
| `wl_seat`（pointer / keyboard） | 入力 | ○ |
| `wp_cursor_shape_v1` | カーソル形状 | 任意 |
| `zwp_text_input_v3` | IME | 任意 |
| `wl_data_device_manager` | クリップボード | 任意 |
| `zwp_primary_selection_v1` | 中クリック貼り付け | 任意 |
| `zwp_linux_dmabuf_v1`（v4 以上） | 自前のバッファ管理（ダメージトラッキング） | 推奨（無ければ wgpu Surface） |
| `wp_linux_drm_syncobj_manager_v1` | 明示的同期 | 任意（無ければ暗黙的同期） |
| `zwp_pointer_gestures_v1`（v3 以上） | 指を置いて慣性を止める | 任意（無ければ次の指の移動で止まる） |

## スケールとサイズ

1. `xdg_toplevel.configure` で論理サイズ、`wp_fractional_scale_v1.preferred_scale` でスケール `n` を受け取る。
2. バッファサイズ = `round(logical × n / 120)`（[01](01-pixel-model.md) R3）。
3. `wp_viewport.set_destination(logical)` を設定し、表示用バッファをバッファサイズで作り直す。
4. どちらも同じ `wl_surface.commit` で反映される。

## イベントループ

```
loop {
    dispatch_pending
    アプリ処理（イベント適用 → effect flush → 必要ならレイアウト・描画）
    flush
    prepare_read → poll([wayland fd, wake pipe], 次のタイマーまでの timeout) → read
}
```

- タイマー: キーリピート、キャレット点滅。発火の判定は、眠る前に求めた締め切りと比べる（`deadline()` は常に「次の」時刻を返すため、起きてから聞き直すと永遠に発火しない）。
- wake pipe: クリップボード読み込みスレッドなど、他スレッドからの通知用。
- フレームコールバック: commit するサーフェスごとに `wl_surface.frame` を要求する。何も提示しないフレーム（変更矩形が空など）では要求も commit もしない。何も変わらない commit にはコンポジタがコールバックを返さないことがあり、待つと描画が止まるため。どのサーフェスにも待ちがないときに次のフレームを描く（ポップアップの待ちは 100 ms で打ち切る）。ただし、ウィンドウだけのときは描画を先に進めて commit だけ待たせることがある（[10](10-frame-pacing.md)）。
- 提示できなかったフレーム（空きバッファが無い）は、数ミリ秒後に再試行する。

## スクロール

`wl_pointer` のイベントは `frame` までまとめて、1 つの `WindowEvent::Scroll` にする（[08](08-scrolling-and-settings.md)）。

| `axis_source` | source | phase |
|---|---|---|
| `finger`（列の最初） | `Touchpad` | `Began` |
| `finger`（続き） | `Touchpad` | `Moved` |
| `axis_stop` のみのフレーム（指を動かしている間だけ） | `Touchpad` | `Lifted { fling: true }` |
| `wheel` / `wheel_tilt`（`axis_value120` あり） | `Wheel` | `Single` |
| それ以外（`continuous`、不明、`axis_value120` なしのホイール） | `Unknown` | `Single` |

- `finger` と `continuous` は移動量を `AXIS_MOTION_GAIN`（5.0）倍して渡す（[08](08-scrolling-and-settings.md)）。
- `axis_stop` が必ず届くのは `finger` だけなので（プロトコルの規定）、他のソースでは慣性を始めない。
- 片方の軸だけの `axis_stop` と、もう片方の軸の移動が同じフレームに来たら、指はまだ動いている。
- `zwp_pointer_gesture_hold_v1` の `begin` は `Held` にする。

## ポップアップ

[09](09-shadows-stacking-popups.md) を参照。ポインタの座標はサーフェスの原点を足してウィンドウ座標にしてから core に渡す。

## キーボード

- `wl_keyboard.keymap` の fd を mmap して xkb keymap を作る。
- `repeat_info` に従いキーリピートを自前で行う。
- `enter` / `leave` は自分のサーフェスの間の移動（grab でポップアップに移る場合など）では core に知らせない。
- core へは `KeyEvent { key: Key（論理キー）, code（物理）, text: Option<String>, modifiers, repeat }` を渡す。

## クリップボード

- コピー: `wl_data_source` に `text/plain;charset=utf-8`, `UTF8_STRING`, `text/plain` を offer し、`send` 要求で fd に書き込む（大きなデータに備えて別スレッド）。
- 貼り付け: 現在の selection offer に `receive` し、読み込みは別スレッドで行って wake pipe で通知する。自分自身が所有する選択の場合は内部バッファから直接返す。
- primary selection も同様。

## IME

- `text_input.enter` を受けたら、フォーカス中の要素が編集可能なら `enable` + 状態送信 + `commit`。
- フォーカスが編集不可能な要素に移ったら `disable` + `commit`。
- 状態変化のたびに `set_surrounding_text` / `set_cursor_rectangle` / `set_content_type` → `commit`。
- `preedit_string` / `commit_string` / `delete_surrounding_text` を溜めておき、`done` でまとめて core に渡す。

## dmabuf presenter

wgpu の Surface（Vulkan WSI）を使わず、表示用バッファを自前で確保して `wl_surface` に直接 attach する。目的は buffer age による部分描画と、`damage_buffer` による変更範囲の通知（[05](05-rendering.md) のダメージトラッキング）。Chromium の Ozone/Wayland と同じ構成。

### バッファの確保

1. `zwp_linux_dmabuf_v1.get_surface_feedback` で、コンポジタが使う GPU（main device）と、受け付ける形式・modifier の一覧（tranche）を受け取る。
2. main device と同じ GPU の Vulkan アダプタを選ぶ（`VK_EXT_physical_device_drm` で dev_t を照合）。見つからなければ wgpu Surface にフォールバックする。
3. `ARGB8888`（Vulkan の `B8G8R8A8_UNORM`）で、コンポジタの tranche と Vulkan の対応 modifier の積集合から `VK_EXT_image_drm_format_modifier` で VkImage を作る（どの modifier を使うかはドライバに選ばせる）。
4. メモリを dmabuf としてエクスポートし（`VK_KHR_external_memory_fd`）、プレーンごとの offset / stride と選ばれた modifier を `zwp_linux_buffer_params_v1` に渡して `wl_buffer` を作る。
5. 同じ VkImage を wgpu-hal 経由で `wgpu::Texture` として取り込み、通常のレンダラで描く。

### 読み込む Vulkan ドライバ

Vulkan ローダーは、インスタンスを作るときに入っているドライバを全部読む。NVIDIA のドライバは、読むだけで D3cold で寝ている dGPU を起こし、起動が約 0.9 秒遅くなっていた（AMD の内蔵 GPU でコンポジタが動くハイブリッド構成。[28](28-markdown.md) の「既存の Markdown ソフトとの比較」）。描くのはどのみち main device なので、ほかの GPU のドライバは読まなくてよい。

- `App::run` の最初、ビューを作ってスレッドが動き出す前に、Wayland に別の接続を短く張り、`get_default_feedback` の `main_device` を受け取る（`anti_webui_wayland::compositor_main_device`、1〜2 ms）。
- その dev_t のドライバを sysfs（`/sys/dev/char/<major>:<minor>/device/driver`）で調べ、そのドライバの Vulkan ドライバのマニフェストだけを `VK_LOADER_DRIVERS_SELECT` に入れる（`anti_webui_wgpu::limit_vulkan_drivers`）。GPU は起こさない。

| main device のドライバ | 読む Vulkan ドライバ（マニフェストの名前に含まれるもの） |
|---|---|
| `amdgpu`、`radeon` | `radeon`（RADV）、`amd_icd`（AMDVLK） |
| `i915`、`xe` | `intel` |
| `nouveau` | `nouveau` |
| `nvidia` | 絞らない（dGPU はコンポジタのために起きている。wgpu Surface のフォールバックが省電力で AMD を選べるように、今のまま） |
| それ以外、分からない | 絞らない |

- 利用者が `VK_LOADER_DRIVERS_SELECT`、`VK_LOADER_DRIVERS_DISABLE`、`VK_DRIVER_FILES`、`VK_ICD_FILENAMES`、`VK_ADD_DRIVER_FILES` を指定しているとき、または dGPU で描かせる指定（`DRI_PRIME`、`__NV_PRIME_RENDER_OFFLOAD`）があるときは、何もしない。
- マニフェストは、ローダーと同じ場所（`$XDG_CONFIG_HOME`、`$XDG_CONFIG_DIRS`、`/etc`、`$XDG_DATA_HOME`、`$XDG_DATA_DIRS` の下の `vulkan/icd.d`）から探し、見つかったものの名前だけを入れる。1 つも見つからなければ絞らない（ドライバが無くなって描けなくなることはない）。
- 環境変数はプロセス全体に効くので、Rust 2024 では `unsafe` の `set_var` を使う。ほかのスレッドが環境変数を読んでいる間に書くのを避けるため、スレッドを作る前の `App::run` の最初に呼ぶ。利用者が `App::run` の前に自分でスレッドを作っていても、ローダーを読む前なら影響は無い。
- `ANTIWEBUI_VULKAN_DRIVERS=all` で絞るのをやめられる（切り分け用）。

バッファは必要に応じて最大 4 枚（保留中のフレームがあればその分を足した枚数）まで増やす。サイズや feedback が変わったら作り直す（古いバッファはコンポジタが返却してから破棄する）。

### フレームの流れ

```
空いているバッファを選ぶ（age が最も小さいもの）
描き直す範囲 = 直近 age フレーム分の変更矩形の和（age 不明なら全面）
所有権を取得（FOREIGN → 自キュー、GENERAL → COLOR_ATTACHMENT）
部分描画
所有権を解放（自キュー → FOREIGN、COLOR_ATTACHMENT → GENERAL）＋ 完了を sync_file として取り出す
attach → damage_buffer（今回の変更矩形）→ 同期点の設定 → commit
```

- キューファミリー所有権の移動は外部メモリを共有するときに Vulkan が要求するもの。AMD の DCC（圧縮）など、ドライバがここで圧縮状態を解決する。
- 不透明な背景のときは `set_opaque_region` を設定し、コンポジタが下のウィンドウの合成を省けるようにする。

### 同期

GPU の描画完了をコンポジタに待たせる仕組みと、コンポジタの読み出し完了を待ってからバッファを再利用する仕組みが必要。CPU で GPU を待つことはしない。

| 方式 | 描画完了の通知 | 再利用の判断 |
|---|---|---|
| 明示的同期（優先） | sync_file を DRM syncobj の acquire タイムラインの点に取り込み、`set_acquire_point` | バッファごとの release タイムラインの点が signal 済みか（`wl_buffer.release` は使えない） |
| 暗黙的同期 | sync_file を dmabuf に取り込む（`DMA_BUF_IOCTL_IMPORT_SYNC_FILE`） | `wl_buffer.release` を受け取り、dmabuf から取り出した sync_file を GPU 側で待つ |

- 描画完了の sync_file は、所有権解放のコマンドに signal させたバイナリセマフォ（`VK_KHR_external_semaphore_fd`）から取り出す。
- syncobj の操作には、Vulkan デバイスに対応する DRM render node を開いて ioctl を使う。
- 空きバッファが無いときは、最も古いバッファの release を短時間だけ待ち、それでも空かなければそのフレームを見送って次の機会に描く。先行して描くフレームのためには待たない。
- `wp_presentation` を使い、commit ごとに画面に出た時刻を受け取る。計測と、先行するかどうかの判断に使う（[10](10-frame-pacing.md)）。

### フォールバック

次の場合は wgpu Surface（全面描画）を使う: `zwp_linux_dmabuf_v1` v4 が無い、Vulkan が使えない（GL バックエンド）、main device に対応するアダプタや必要な拡張が無い、バッファ作成に失敗した。環境変数 `ANTIWEBUI_PRESENT=surface` で強制、`ANTIWEBUI_SYNC=implicit` で暗黙的同期に固定できる。
