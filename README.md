# U3 OrderFlow

U3 OrderFlow 是一款專為加密貨幣市場打造的高效能原生桌面看盤軟體。
提供專業的訂單流 (Orderflow) 分析與進階指標，幫助交易者精準掌握市場動態。

## 🌟 核心特色 (Key Features)

* **自訂專屬介面**：全新升級的 U3 OrderFlow 專屬介面與主題配色。
* **高階訂單流指標**：
  * **Delta Bar**：以底部柱狀圖直觀呈現 K 線的多空力道 (Delta)。
  * **Session Delta Wave (累積 CVD)**：每日台灣時間早上 8 點 (00:00 UTC) 自動重置的波浪柱狀圖，清晰顯示當日累積的多空資金流向。
  * **CVD Divergence (背離提示)**：當價格與 Delta 發生背離時，直接於 K 線圖中央顯示紅綠箭頭提示，精準捕捉反轉訊號。
  * **VWAP**：整合於主圖的成交量加權平均價指標。
* **極簡流暢體驗**：移除不必要的多餘指標，確保軟體運行極致順暢且介面清爽。
* **多交易所支援**：支援 Binance, Bybit, Hyperliquid, OKX, 與 MEXC。

## 🚀 開始使用

本專案使用 [Rust](https://www.rust-lang.org/) 語言開發。

### 編譯與執行

確保您已安裝 Rust 工具鏈，然後在專案根目錄執行：

```bash
cargo run --release
```

這將會編譯並啟動 U3 OrderFlow。

---
*Powered by U3 OrderFlow*
