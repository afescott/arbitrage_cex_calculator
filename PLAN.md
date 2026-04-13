Cross-venue spread (price difference arb)
    
  Idea: If BTC-PERP is slightly higher on Venue A than Venue B, you short the higher and long the lower, aiming to capture convergence.
  Risk: The spread can widen before it converges; you can get liquidated on one side if you run tight margin.
  Hard part: You need fast execution and stable hedging and low fees. With small capital, execution costs dominate.

Stablecoin collateral / chain: You need USDC/USDT on whatever deposit/collateral rails each venue uses, and for cross-venue you ideally have funds already sitting on both venues so you’re not waiting on bridges/transfers mid-trade.
Neutral vs directional: Neutral (hedged) is better for cross-venue spread because it isolates the spread, but it’s more difficult since you must fill and maintain both legs without one side running away or getting liquidated.



