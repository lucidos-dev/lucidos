import { request } from './_fetch';

export interface PriceRates {
  uncached_in: number;
  cache_write: number;
  cache_write_1h?: number;
  cache_read: number;
  out: number;
}

export interface PriceCard extends PriceRates {
  from?: string;
  long?: PriceRates & { threshold_tokens: number; inclusive?: boolean };
  flex?: PriceRates;
  per_minute?: number;
  source: string;
  origin: 'shipped' | 'override';
}

export interface PriceTable {
  currency: string;
  checked: string;
  unpriced_cache_shares: { read: number; write_5m: number; write_1h: number };
  models: Record<string, PriceCard[]>;
  override: {
    path: string;
    present: boolean;
    checked: string | null;
    error: string | null;
  };
}

export const priceTable = {
  get(): Promise<PriceTable> {
    return request('/model-prices');
  },
};
