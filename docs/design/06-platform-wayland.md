# 06. Wayland バックエンド

`wayland-client` + `wayland-protocols` を直接使う（ツールキットは使わない）。キーボードは `libxkbcommon`。

## 使用プロトコル

| プロトコル | 用途 | 必須 |
|---|---|---|
| `wl_compositor`, `xdg_wm_base` | ウィンドウ | ○ |
| `wp_fractional_scale_v1` + `wp_viewporter` | 小数スケール | 推奨（無ければ `preferred_buffer_scale` の整数スケール） |
| `zxdg_decoration_manager_v1` | サーバ側装飾 | 推奨（無い場合は装飾なし。CSD は将来） |
| `wl_seat`（pointer / keyboard） | 入力 | ○ |
| `wp_cursor_shape_v1` | カーソル形状 | 任意 |
| `zwp_text_input_v3` | IME | 任意 |
| `wl_data_device_manager` | クリップボード | 任意 |
| `zwp_primary_selection_v1` | 中クリック貼り付け | 任意 |
| `zwp_linux_dmabuf_v1`（v4 以上） | 自前のバッファ管理（ダメージトラッキング） | 推奨（無ければ wgpu Surface） |
| `wp_linux_drm_syncobj_manager_v1` | 明示的同期 | 任意（無ければ暗黙的同期） |

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
- フレームコールバック: 描画のたびに `wl_surface.frame` を要求するが、何も提示しなかったフレーム（変更矩形が空など）では commit せず、要求は次の commit まで保留する。何も変わらない commit にはコンポジタがコールバックを返さないことがあり、待つと描画が止まるため。
- 提示できなかったフレーム（空きバッファが無い）は、数ミリ秒後に再試行する。

## キーボード

- `wl_keyboard.keymap` の fd を mmap して xkb keymap を作る。
- `repeat_info` に従いキーリピートを自前で行う。
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

バッファは必要に応じて最大 4 枚まで増やす。サイズや feedback が変わったら作り直す（古いバッファはコンポジタが返却してから破棄する）。

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
- 空きバッファが無いときは、最も古いバッファの release を短時間だけ待ち、それでも空かなければそのフレームを見送って次の機会に描く。

### フォールバック

次の場合は wgpu Surface（全面描画）を使う: `zwp_linux_dmabuf_v1` v4 が無い、Vulkan が使えない（GL バックエンド）、main device に対応するアダプタや必要な拡張が無い、バッファ作成に失敗した。環境変数 `ANTIWEBUI_PRESENT=surface` で強制、`ANTIWEBUI_SYNC=implicit` で暗黙的同期に固定できる。
