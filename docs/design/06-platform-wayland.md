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

## スケールとサイズ

1. `xdg_toplevel.configure` で論理サイズ、`wp_fractional_scale_v1.preferred_scale` でスケール `n` を受け取る。
2. バッファサイズ = `round(logical × n / 120)`（[01](01-pixel-model.md) R3）。
3. `wp_viewport.set_destination(logical)` を設定し、wgpu サーフェスをバッファサイズで再構成する。
4. どちらも同じ `wl_surface.commit`（WSI の present）で反映される。

## イベントループ

```
loop {
    dispatch_pending
    アプリ処理（イベント適用 → effect flush → 必要ならレイアウト・描画）
    flush
    prepare_read → poll([wayland fd, wake pipe], 次のタイマーまでの timeout) → read
}
```

- タイマー: キーリピート、キャレット点滅。
- wake pipe: クリップボード読み込みスレッドなど、他スレッドからの通知用。

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
