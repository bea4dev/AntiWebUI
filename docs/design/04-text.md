# 04. テキスト

## 役割分担

| 処理 | 担当 |
|---|---|
| フォント列挙・フォールバック | fontique（parley 経由、Linux では fontconfig 設定を参照） |
| シェイピング・双方向テキスト・行分割 | parley（`quantize = true`） |
| カーソル移動の基本操作（視覚順、単語、行） | parley の `Cursor` / `Selection` |
| **選択モデル（ドキュメント横断）** | AntiWebUI（core） |
| **編集モデル・IME・クリップボード連携** | AntiWebUI（core） |
| グリフのラスタライズ・アトラス | AntiWebUI（wgpu、ラスタライズは swash） |

ロケールは環境変数 `LANG` から取得して parley に渡す（日本語環境では CJK フォールバックが日本語字形になる）。

## 選択モデル

ブラウザと同じく **ドキュメント全体で 1 つ** の選択を持つ。

```rust
struct TextPoint { node: NodeId, offset: usize /* バイト位置（書記素境界） */ }
struct DocSelection { anchor: TextPoint, focus: TextPoint }
```

- 文書順はレイアウトのたびにツリーを前順走査して `doc_order: u32` を振り、`(doc_order, offset)` で比較する。
- 選択可能かどうかはスタイルの `user_select`（既定: テキストは `Text`、ボタンなどは `None`）で決まり、継承する。
- 入力欄（TextEdit）は独自の選択を持つ（ブラウザの `<input>` と同じ）。

### ヒットテスト（最も近いテキスト位置）

ドラッグ中のポインタが要素の外・行間・余白にあっても、ブラウザのように自然に選択できるよう、次の順で最も近い位置を求める。

1. 選択可能なテキストノードのうち、ポインタを含む行ボックスを持つものがあれば、その中で `Cursor::from_point`。
2. なければ、各テキストノードの可視矩形との距離を「縦方向の距離 → 横方向の距離」の辞書式順序で比較し、最も近いノードを選ぶ。ポインタをそのノードの矩形内にクランプしてから `Cursor::from_point`。
3. 選択開始ノードより文書順で前に戻ることも、離れた要素にまたがることも自由。

### ダブル / トリプルクリック

- ダブルクリック: 単語選択（parley の単語境界、日本語は辞書ベース）。ドラッグすると単語単位で拡張。
- トリプルクリック: 段落（ハードラインブレーク区切り）選択。ドラッグすると段落単位で拡張。

### ハイライト描画

各テキストノードについて、選択範囲と重なる部分の矩形を `Selection::geometry` で求める。y 方向は行ボックス（整数）にスナップするため、複数行のハイライトは隙間も重なりもなく接する。改行をまたぐ場合は行末に小さな余白を加える（ブラウザ同様）。

### コピー

文書順に各ノードの部分文字列を連結する。ノード間には、次のノードが視覚的に新しい行で始まるなら `\n`、同じ行なら ` ` を挟む。選択が確定したら Wayland の primary selection にも設定する。

## 編集モデル（TextEdit）

```rust
struct EditState {
    text: String,
    selection: (usize, usize),        // anchor, focus（バイト位置）
    preedit: Option<Preedit>,          // IME の未確定文字列
    scroll: (i32, i32),                // 内部スクロール（物理 px）
    undo: UndoStack,
}
```

- 単一行（`text_input`）と複数行（`text_area`）。
- キー操作: 矢印（視覚順・Shift で拡張・Ctrl で単語）、Home/End、上下（水平位置を保持）、Backspace/Delete（書記素・Ctrl で単語）、Ctrl+A/C/X/V/Z/Shift+Z、Enter（複数行では改行、単一行では submit イベント）。
- マウス: クリック・ドラッグ・ダブル/トリプルクリック・Shift+クリック、中クリックで primary selection を貼り付け。
- キャレットが常に見えるよう内部スクロールを調整する。キャレットは 500ms で点滅し、入力のたびにリセットする。
- `Signal<String>` と双方向バインドできる（編集 → signal 書き込み、signal の外部変更 → テキスト更新）。

## IME（text-input-v3）

- 未確定文字列（preedit）は、キャレット位置に **一時的に挿入した状態でレイアウト** し、下線を引いて描画する。IME が指定する preedit 内カーソル範囲も表示する。
- preedit 開始時に選択範囲があれば削除する。
- `done` イベントで次の順に適用する: (1) 既存 preedit を除去 (2) `delete_surrounding_text` (3) `commit_string` を挿入 (4) 新しい preedit を設定。適用後、周辺テキスト・カーソル矩形を送って `commit`。
- 周辺テキスト（surrounding text）はキャレット周辺の最大 4000 バイト。
- カーソル矩形は論理座標に変換して送る（[01](01-pixel-model.md) R7）。

core は IME 状態を `ImeRequest { enabled, surrounding, cursor_rect, content_type }` として出力し、プラットフォーム層がプロトコルに変換する。
