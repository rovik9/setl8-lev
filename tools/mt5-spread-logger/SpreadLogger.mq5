//+------------------------------------------------------------------+
//| SpreadLogger.mq5  (setl8-lev-trading, tools/mt5-spread-logger)   |
//| Read-only MT5 script: every InpSampleSec seconds it writes one   |
//| row per symbol (UTC time, bid, ask) to a CSV in the Common\Files |
//| folder, plus one specification file at start. It never trades,   |
//| never sends an order, never reads account data.                  |
//+------------------------------------------------------------------+
#property copyright "setl8"
#property version   "1.00"
#property strict
#property script_show_inputs

// EDIT the names to match the Market Watch of YOUR Exness account (the suffix depends on the account type).
input string InpSymbols   = "BTCUSD,ETHUSD,SOLUSD,XAUUSD,XAGUSD,USOIL,UKOIL,US500,USTEC,JP225,AAPL,AMD,AMZN,GOOGL,META,MSFT,NFLX,NVDA,PLTR,TSLA";
input int    InpSampleSec = 5;
input string InpTag       = "exness_standard";   // free text: account type, written into the file name

string g_syms[];

void OnStart()
{
   int n = StringSplit(InpSymbols, ',', g_syms);
   if(n <= 0) { Print("no symbols"); return; }
   for(int i = 0; i < n; i++)
   {
      StringTrimLeft(g_syms[i]);
      StringTrimRight(g_syms[i]);
      if(!SymbolSelect(g_syms[i], true))
         PrintFormat("symbol not found in Market Watch: %s (fix the name)", g_syms[i]);
   }

   string stamp = TimeToString(TimeGMT(), TIME_DATE | TIME_MINUTES);
   StringReplace(stamp, ".", "");
   StringReplace(stamp, ":", "");
   StringReplace(stamp, " ", "_");

   // 1. specification file (written once)
   string spec_name = "setl8_" + InpTag + "_spec_" + stamp + ".csv";
   int fs = FileOpen(spec_name, FILE_WRITE | FILE_CSV | FILE_ANSI | FILE_COMMON, ',');
   if(fs == INVALID_HANDLE) { PrintFormat("cannot open %s, error %d", spec_name, GetLastError()); return; }
   FileWrite(fs, "symbol", "digits", "contract_size", "tick_size", "tick_value", "volume_min", "volume_step", "volume_max", "spread_float", "calc_mode", "currency_profit", "description");
   for(int i = 0; i < n; i++)
   {
      string s = g_syms[i];
      FileWrite(fs, s,
                (string)SymbolInfoInteger(s, SYMBOL_DIGITS),
                DoubleToString(SymbolInfoDouble(s, SYMBOL_TRADE_CONTRACT_SIZE), 8),
                DoubleToString(SymbolInfoDouble(s, SYMBOL_TRADE_TICK_SIZE), 10),
                DoubleToString(SymbolInfoDouble(s, SYMBOL_TRADE_TICK_VALUE), 10),
                DoubleToString(SymbolInfoDouble(s, SYMBOL_VOLUME_MIN), 8),
                DoubleToString(SymbolInfoDouble(s, SYMBOL_VOLUME_STEP), 8),
                DoubleToString(SymbolInfoDouble(s, SYMBOL_VOLUME_MAX), 8),
                (string)SymbolInfoInteger(s, SYMBOL_SPREAD_FLOAT),
                (string)SymbolInfoInteger(s, SYMBOL_TRADE_CALC_MODE),
                SymbolInfoString(s, SYMBOL_CURRENCY_PROFIT),
                SymbolInfoString(s, SYMBOL_DESCRIPTION));
   }
   FileClose(fs);

   // 2. sample file (one row per symbol per InpSampleSec, flushed every round)
   string name = "setl8_" + InpTag + "_samples_" + stamp + ".csv";
   int f = FileOpen(name, FILE_WRITE | FILE_READ | FILE_CSV | FILE_ANSI | FILE_COMMON | FILE_SHARE_READ, ',');
   if(f == INVALID_HANDLE) { PrintFormat("cannot open %s, error %d", name, GetLastError()); return; }
   FileSeek(f, 0, SEEK_END);
   if(FileSize(f) == 0)
      FileWrite(f, "utc", "server_time_msc", "symbol", "bid", "ask", "tick_age_s");
   PrintFormat("SpreadLogger running: %d symbols, every %d s, file %s (Common\\Files)", n, InpSampleSec, name);

   while(!IsStopped())
   {
      datetime now = TimeGMT();
      string utc = TimeToString(now, TIME_DATE | TIME_SECONDS);
      for(int i = 0; i < n; i++)
      {
         MqlTick t;
         if(!SymbolInfoTick(g_syms[i], t)) continue;
         if(t.bid <= 0 || t.ask <= 0) continue;
         int digits = (int)SymbolInfoInteger(g_syms[i], SYMBOL_DIGITS);
         long age = (long)(TimeCurrent() - t.time);
         FileWrite(f, utc, (string)t.time_msc, g_syms[i],
                   DoubleToString(t.bid, digits), DoubleToString(t.ask, digits), (string)age);
      }
      FileFlush(f);
      Sleep(InpSampleSec * 1000);
   }
   FileClose(f);
   Print("SpreadLogger stopped");
}
