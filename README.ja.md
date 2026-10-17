<div align="center">

# mixerplus

**Windows 11 のサウンドのフライアウトそっくりな、小さなトレイ音量ミキサー。**

[![Rust](https://img.shields.io/badge/Rust-native%20Win32-DEA584?logo=rust&logoColor=black)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue)](LICENSE)

[English](README.md) · **日本語**

<img src="docs/images/popup.png" alt="出力・入力・アプリごとのスライダーが並ぶ mixerplus のポップアップ" width="300">

<sub>ポップアップを <code>--snapshot</code> で書き出したものです（<a href="DEVELOPMENT.md">DEVELOPMENT.md</a> 参照）。</sub>

</div>

---

## 特長

- **ワンクリックで調整**: トレイアイコンをクリックすると、既定の出力と入力、そしてアプリごとの音量を調整できます。どのスライダーにも、音量の値とリアルタイムのレベルメーターが付きます。
- **アプリごとにミュート**: アプリのアイコンをクリックするだけです。
- **デバイスの切り替え**: 出力・入力の欄には既定のデバイスが表示されます。シェブロンで展開するとすべてのデバイスが並び、既定のデバイスを選んだり、デバイスごとの音量を変えたりできます。開いたままか閉じたままかは前回の状態を覚えています。展開した出力の欄では立体音響（Windows Sonic for Headphones）も切り替えられます。
- **スクロール**: 音を出しているアプリが多いときは、マウスホイールやスクロールバーのドラッグでミキサーをスクロールできます。
- **プリセット**: 今の音量とミュートの状態（出力、入力、すべてのアプリ）に名前を付けて保存し、あとでワンクリックで適用できます。自動で適用されることはありません。
- **ネイティブな見た目**: システムのライト／ダークモードとアクセントカラーに従います。純粋な Win32 で、追加のランタイムは要りません。
- **オープンソースのクレジット**: トレイアイコンを右クリックすると、「mixerplus について」でライセンスを、「Credits」で組み込んだライブラリの一覧とそれぞれのライセンスを見られます。

## インストール

```powershell
irm https://github.com/valneko-twoon2/mixerplus/releases/latest/download/install.ps1 | iex
```

現在のユーザー用に（管理者権限なしで）`%LocalAppData%\Programs\mixerplus` へ入れて起動し、サインインのたびに自動で起動します。更新するときは、同じコマンドをもう一度実行します。

削除するには、設定の「アプリ」を使います。プリセット（`%AppData%\mixerplus\presets.json`）は、インストール先の `uninstall.ps1` を `-RemoveData` 付きで実行しない限り残ります。

## 注意

- 一覧に出るのは、今、音声セッションを持っているアプリだけです。排他モードで再生しているアプリには、アプリの音量が効かないことがあります。
- 既定のデバイスの切り替えには、文書化されていない Windows のインターフェイスを使っているため、Windows の更新で動かなくなることがあります。
- mixerplus が書き込むのはプリセットのファイルだけで、音声を録音することはありません。

ソースからのビルドは、[DEVELOPMENT.md](DEVELOPMENT.md)（英語）を見てください。

## ライセンス

MIT。[LICENSE](LICENSE) を参照してください。サードパーティのライセンスは [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt) と、アプリ内（トレイアイコンの右クリック > 「Credits」）にあります。
