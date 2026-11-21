import { request } from './_fetch';

/** One tier's rates, per million tokens in the table's currency. */
export interface PriceRates {
  uncached_in: number;
  /** The 5-minute cache write. */
  cache_write: number;
  /** The 1-hour cache write. Omitted means `cache_write`. */
  cache_write_1h?: number;
  cache_read: number;
  out: number;
}

/** One dated price of one model, in Token Cost's card format. */
export interface PriceCard extends PriceRates {
  /** The first day the card is in force (YYYY-MM-DD, local time). Omitted
   *  means since the beginning. */
  from?: string;
  /** The long-context tier: its rates replace the base ones for every token
   *  of a call whose prompt reached `threshold_tokens`. */
  long?: PriceRates & { threshold_tokens: number; inclusive?: boolean };
  /** The Flex tier's rates. */
  flex?: PriceRates;
  /** A model billed by wall-clock minute. Its token rates are then unused. */
  per_minute?: number;
  source: string;
  /** `shipped` with Lucidos, or `override` from `data/config/model-prices.json`. */
  origin: 'shipped' | 'override';
}

/** The price table: the shipped cards merged with the workspace override. */
export interface PriceTable {
  currency: string;
  /** The day the prices in force were last checked, `YYYY-MM-DD`: the
   *  override's day when it is newer than the shipped one. */
  checked: string;
  /** What an unpriced model's cache costs, as shares of its input price. */
  unpriced_cache_shares: { read: number; write_5m: number; write_1h: number };
  /** Each model's cards, oldest first. */
  models: Record<string, PriceCard[]>;
  override: {
    path: string;
    present: boolean;
    /** The day the override's prices were last checked, as it records. */
    checked: string | null;
    /** Why the override was ignored. The shipped cards then stand alone. */
    error: string | null;
  };
}

export const priceTable = {
  get(): Promise<PriceTable> {
    return request('/model-prices');
  },
};
