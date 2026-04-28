import type { CurrencySettings } from '../types'

// 货币符号映射
const CURRENCY_SYMBOLS: Record<string, string> = {
  USD: '$',
  CNY: '¥',
  EUR: '€',
  GBP: '£',
  JPY: '¥',
  KRW: '₩',
  HKD: 'HK$',
  TWD: 'NT$',
  AUD: 'A$',
  CAD: 'C$',
  SGD: 'S$',
  CHF: 'CHF ',
  INR: '₹',
  RUB: '₽',
  BRL: 'R$',
  MXN: 'MX$',
  THB: '฿',
  MYR: 'RM',
  IDR: 'Rp',
  PHP: '₱',
  VND: '₫',
  AED: 'AED ',
  SAR: 'SAR ',
  TRY: '₺',
  SEK: 'kr ',
  NOK: 'kr ',
  DKK: 'kr ',
  PLN: 'zł ',
  ZAR: 'R ',
  NGN: '₦',
  ARS: 'ARS$',
  CLP: 'CLP$',
  COP: 'COL$',
  EGP: 'EGP ',
  ILS: '₪',
  NZD: 'NZ$',
  PKR: 'Rs ',
  CZK: 'Kč',
  HUF: 'Ft',
  RON: 'lei',
  BGN: 'лв',
  HRK: 'kn',
  ISK: 'kr',
  UAH: '₴',
  KES: 'KSh',
  GHS: 'GH₵',
  UGX: 'USh',
  TZS: 'TSh',
  ETB: 'Br',
  ZMW: 'ZK',
  BDT: '৳',
  LKR: 'Rs',
  NPR: 'रू',
  AFN: '؋',
  AMD: '֏',
  AZN: '₼',
  BND: 'B$',
  DZD: 'د.ج',
  IRR: '﷼',
  IQD: 'ع.د',
  JOD: 'JD',
  KWD: 'د.ك',
  LBP: 'ل.ل',
  LYD: 'ل.د',
  MAD: 'د.م.',
  OMR: 'ر.ع.',
  QAR: 'ر.ق',
  SYP: '£',
  TND: 'د.ت',
  YER: '﷼',
}

// 货币名称国际化映射
const CURRENCY_NAMES: Record<string, { 'zh-CN': string; 'zh-TW': string; 'en-US': string }> = {
  USD: { 'zh-CN': '美元', 'zh-TW': '美元', 'en-US': 'US Dollar' },
  CNY: { 'zh-CN': '人民币', 'zh-TW': '人民幣', 'en-US': 'Chinese Yuan' },
  EUR: { 'zh-CN': '欧元', 'zh-TW': '歐元', 'en-US': 'Euro' },
  GBP: { 'zh-CN': '英镑', 'zh-TW': '英鎊', 'en-US': 'British Pound' },
  JPY: { 'zh-CN': '日元', 'zh-TW': '日圓', 'en-US': 'Japanese Yen' },
  KRW: { 'zh-CN': '韩元', 'zh-TW': '韓元', 'en-US': 'South Korean Won' },
  HKD: { 'zh-CN': '港币', 'zh-TW': '港幣', 'en-US': 'Hong Kong Dollar' },
  TWD: { 'zh-CN': '新台币', 'zh-TW': '新台幣', 'en-US': 'New Taiwan Dollar' },
  AUD: { 'zh-CN': '澳元', 'zh-TW': '澳元', 'en-US': 'Australian Dollar' },
  CAD: { 'zh-CN': '加元', 'zh-TW': '加元', 'en-US': 'Canadian Dollar' },
  SGD: { 'zh-CN': '新加坡元', 'zh-TW': '新加坡元', 'en-US': 'Singapore Dollar' },
  CHF: { 'zh-CN': '瑞士法郎', 'zh-TW': '瑞士法郎', 'en-US': 'Swiss Franc' },
  INR: { 'zh-CN': '印度卢比', 'zh-TW': '印度盧比', 'en-US': 'Indian Rupee' },
  RUB: { 'zh-CN': '俄罗斯卢布', 'zh-TW': '俄羅斯盧布', 'en-US': 'Russian Ruble' },
  BRL: { 'zh-CN': '巴西雷亚尔', 'zh-TW': '巴西雷亞爾', 'en-US': 'Brazilian Real' },
  MXN: { 'zh-CN': '墨西哥比索', 'zh-TW': '墨西哥披索', 'en-US': 'Mexican Peso' },
  THB: { 'zh-CN': '泰铢', 'zh-TW': '泰銖', 'en-US': 'Thai Baht' },
  MYR: { 'zh-CN': '马来西亚林吉特', 'zh-TW': '馬來西亞令吉', 'en-US': 'Malaysian Ringgit' },
  IDR: { 'zh-CN': '印尼盾', 'zh-TW': '印尼盾', 'en-US': 'Indonesian Rupiah' },
  PHP: { 'zh-CN': '菲律宾比索', 'zh-TW': '菲律賓披索', 'en-US': 'Philippine Peso' },
  VND: { 'zh-CN': '越南盾', 'zh-TW': '越南盾', 'en-US': 'Vietnamese Dong' },
  AED: { 'zh-CN': '阿联酋迪拉姆', 'zh-TW': '阿聯酋迪拉姆', 'en-US': 'UAE Dirham' },
  SAR: { 'zh-CN': '沙特里亚尔', 'zh-TW': '沙烏地里亞爾', 'en-US': 'Saudi Riyal' },
  TRY: { 'zh-CN': '土耳其里拉', 'zh-TW': '土耳其里拉', 'en-US': 'Turkish Lira' },
  SEK: { 'zh-CN': '瑞典克朗', 'zh-TW': '瑞典克朗', 'en-US': 'Swedish Krona' },
  NOK: { 'zh-CN': '挪威克朗', 'zh-TW': '挪威克朗', 'en-US': 'Norwegian Krone' },
  DKK: { 'zh-CN': '丹麦克朗', 'zh-TW': '丹麥克朗', 'en-US': 'Danish Krone' },
  PLN: { 'zh-CN': '波兰兹罗提', 'zh-TW': '波蘭茲羅提', 'en-US': 'Polish Zloty' },
  ZAR: { 'zh-CN': '南非兰特', 'zh-TW': '南非蘭特', 'en-US': 'South African Rand' },
  NGN: { 'zh-CN': '尼日利亚奈拉', 'zh-TW': '奈及利亞奈拉', 'en-US': 'Nigerian Naira' },
  ARS: { 'zh-CN': '阿根廷比索', 'zh-TW': '阿根廷披索', 'en-US': 'Argentine Peso' },
  CLP: { 'zh-CN': '智利比索', 'zh-TW': '智利披索', 'en-US': 'Chilean Peso' },
  COP: { 'zh-CN': '哥伦比亚比索', 'zh-TW': '哥倫比亞披索', 'en-US': 'Colombian Peso' },
  EGP: { 'zh-CN': '埃及镑', 'zh-TW': '埃及鎊', 'en-US': 'Egyptian Pound' },
  ILS: { 'zh-CN': '以色列新谢克尔', 'zh-TW': '以色列新謝克爾', 'en-US': 'Israeli Shekel' },
  NZD: { 'zh-CN': '新西兰元', 'zh-TW': '紐西蘭元', 'en-US': 'New Zealand Dollar' },
  PKR: { 'zh-CN': '巴基斯坦卢比', 'zh-TW': '巴基斯坦盧比', 'en-US': 'Pakistani Rupee' },
  CZK: { 'zh-CN': '捷克克朗', 'zh-TW': '捷克克朗', 'en-US': 'Czech Koruna' },
  HUF: { 'zh-CN': '匈牙利福林', 'zh-TW': '匈牙利福林', 'en-US': 'Hungarian Forint' },
  RON: { 'zh-CN': '罗马尼亚列伊', 'zh-TW': '羅馬尼亞列伊', 'en-US': 'Romanian Leu' },
  BGN: { 'zh-CN': '保加利亚列弗', 'zh-TW': '保加利亞列弗', 'en-US': 'Bulgarian Lev' },
  HRK: { 'zh-CN': '克罗地亚库纳', 'zh-TW': '克羅埃西亞庫納', 'en-US': 'Croatian Kuna' },
  ISK: { 'zh-CN': '冰岛克朗', 'zh-TW': '冰島克朗', 'en-US': 'Icelandic Krona' },
  UAH: { 'zh-CN': '乌克兰格里夫纳', 'zh-TW': '烏克蘭格里夫納', 'en-US': 'Ukrainian Hryvnia' },
  KES: { 'zh-CN': '肯尼亚先令', 'zh-TW': '肯亞先令', 'en-US': 'Kenyan Shilling' },
  GHS: { 'zh-CN': '加纳塞地', 'zh-TW': '迦納塞地', 'en-US': 'Ghanaian Cedi' },
  UGX: { 'zh-CN': '乌干达先令', 'zh-TW': '烏干達先令', 'en-US': 'Ugandan Shilling' },
  TZS: { 'zh-CN': '坦桑尼亚先令', 'zh-TW': '坦尚尼亞先令', 'en-US': 'Tanzanian Shilling' },
  ETB: { 'zh-CN': '埃塞俄比亚比尔', 'zh-TW': '衣索比亞比爾', 'en-US': 'Ethiopian Birr' },
  ZMW: { 'zh-CN': '赞比亚克瓦查', 'zh-TW': '尚比亞克瓦查', 'en-US': 'Zambian Kwacha' },
  BDT: { 'zh-CN': '孟加拉塔卡', 'zh-TW': '孟加拉塔卡', 'en-US': 'Bangladeshi Taka' },
  LKR: { 'zh-CN': '斯里兰卡卢比', 'zh-TW': '斯里蘭卡盧比', 'en-US': 'Sri Lankan Rupee' },
  NPR: { 'zh-CN': '尼泊尔卢比', 'zh-TW': '尼泊爾盧比', 'en-US': 'Nepalese Rupee' },
  AFN: { 'zh-CN': '阿富汗尼', 'zh-TW': '阿富汗尼', 'en-US': 'Afghan Afghani' },
  AMD: { 'zh-CN': '亚美尼亚德拉姆', 'zh-TW': '亞美尼亞德拉姆', 'en-US': 'Armenian Dram' },
  AZN: { 'zh-CN': '阿塞拜疆马纳特', 'zh-TW': '亞塞拜然馬納特', 'en-US': 'Azerbaijani Manat' },
  BND: { 'zh-CN': '文莱元', 'zh-TW': '汶萊元', 'en-US': 'Brunei Dollar' },
  DZD: { 'zh-CN': '阿尔及利亚第纳尔', 'zh-TW': '阿爾及利亞第納爾', 'en-US': 'Algerian Dinar' },
  IRR: { 'zh-CN': '伊朗里亚尔', 'zh-TW': '伊朗里亞爾', 'en-US': 'Iranian Rial' },
  IQD: { 'zh-CN': '伊拉克第纳尔', 'zh-TW': '伊拉克第納爾', 'en-US': 'Iraqi Dinar' },
  JOD: { 'zh-CN': '约旦第纳尔', 'zh-TW': '約旦第納爾', 'en-US': 'Jordanian Dinar' },
  KWD: { 'zh-CN': '科威特第纳尔', 'zh-TW': '科威特第納爾', 'en-US': 'Kuwaiti Dinar' },
  LBP: { 'zh-CN': '黎巴嫩镑', 'zh-TW': '黎巴嫩鎊', 'en-US': 'Lebanese Pound' },
  LYD: { 'zh-CN': '利比亚第纳尔', 'zh-TW': '利比亞第納爾', 'en-US': 'Libyan Dinar' },
  MAD: { 'zh-CN': '摩洛哥迪拉姆', 'zh-TW': '摩洛哥迪拉姆', 'en-US': 'Moroccan Dirham' },
  OMR: { 'zh-CN': '阿曼里亚尔', 'zh-TW': '阿曼里亞爾', 'en-US': 'Omani Rial' },
  QAR: { 'zh-CN': '卡塔尔里亚尔', 'zh-TW': '卡達里亞爾', 'en-US': 'Qatari Riyal' },
  SYP: { 'zh-CN': '叙利亚镑', 'zh-TW': '敘利亞鎊', 'en-US': 'Syrian Pound' },
  TND: { 'zh-CN': '突尼斯第纳尔', 'zh-TW': '突尼西亞第納爾', 'en-US': 'Tunisian Dinar' },
  YER: { 'zh-CN': '也门里亚尔', 'zh-TW': '葉門里亞爾', 'en-US': 'Yemeni Rial' },
}

const EXCHANGE_RATE_API_CURRENCY_CODES = [
  'USD', 'AED', 'AFN', 'ALL', 'AMD', 'ANG', 'AOA', 'ARS', 'AUD', 'AWG', 'AZN', 'BAM',
  'BBD', 'BDT', 'BGN', 'BHD', 'BIF', 'BMD', 'BND', 'BOB', 'BRL', 'BSD', 'BTN', 'BWP',
  'BYN', 'BZD', 'CAD', 'CDF', 'CHF', 'CLF', 'CLP', 'CNH', 'CNY', 'COP', 'CRC', 'CUP',
  'CVE', 'CZK', 'DJF', 'DKK', 'DOP', 'DZD', 'EGP', 'ERN', 'ETB', 'EUR', 'FJD', 'FKP',
  'FOK', 'GBP', 'GEL', 'GGP', 'GHS', 'GIP', 'GMD', 'GNF', 'GTQ', 'GYD', 'HKD', 'HNL',
  'HRK', 'HTG', 'HUF', 'IDR', 'ILS', 'IMP', 'INR', 'IQD', 'IRR', 'ISK', 'JEP', 'JMD',
  'JOD', 'JPY', 'KES', 'KGS', 'KHR', 'KID', 'KMF', 'KRW', 'KWD', 'KYD', 'KZT', 'LAK',
  'LBP', 'LKR', 'LRD', 'LSL', 'LYD', 'MAD', 'MDL', 'MGA', 'MKD', 'MMK', 'MNT', 'MOP',
  'MRU', 'MUR', 'MVR', 'MWK', 'MXN', 'MYR', 'MZN', 'NAD', 'NGN', 'NIO', 'NOK', 'NPR',
  'NZD', 'OMR', 'PAB', 'PEN', 'PGK', 'PHP', 'PKR', 'PLN', 'PYG', 'QAR', 'RON', 'RSD',
  'RUB', 'RWF', 'SAR', 'SBD', 'SCR', 'SDG', 'SEK', 'SGD', 'SHP', 'SLE', 'SLL', 'SOS',
  'SRD', 'SSP', 'STN', 'SYP', 'SZL', 'THB', 'TJS', 'TMT', 'TND', 'TOP', 'TRY', 'TTD',
  'TVD', 'TWD', 'TZS', 'UAH', 'UGX', 'UYU', 'UZS', 'VES', 'VND', 'VUV', 'WST', 'XAF',
  'XCD', 'XCG', 'XDR', 'XOF', 'XPF', 'YER', 'ZAR', 'ZMW', 'ZWG', 'ZWL'
] as const

function normalizedCurrencyLocale(locale: string): 'zh-CN' | 'zh-TW' | 'en-US' {
  return locale === 'en-US' ? 'en-US' : locale === 'zh-TW' ? 'zh-TW' : 'zh-CN'
}

export function getCurrencySymbol(code: string): string {
  if (CURRENCY_SYMBOLS[code]) return CURRENCY_SYMBOLS[code]

  try {
    const parts = new Intl.NumberFormat('en-US', {
      style: 'currency',
      currency: code,
      currencyDisplay: 'narrowSymbol',
      minimumFractionDigits: 0,
      maximumFractionDigits: 0
    }).formatToParts(0)
    const symbol = parts.find(part => part.type === 'currency')?.value
    return symbol && symbol !== code ? symbol : `${code} `
  } catch {
    return `${code} `
  }
}

export function getCurrencyName(code: string, locale: string): string {
  const names = CURRENCY_NAMES[code]
  const normalizedLocale = normalizedCurrencyLocale(locale)
  if (names) return names[normalizedLocale]

  try {
    const DisplayNames = (Intl as typeof Intl & {
      DisplayNames?: new (locales: string | string[], options: { type: 'currency' }) => { of: (code: string) => string | undefined }
    }).DisplayNames
    return DisplayNames ? new DisplayNames(normalizedLocale, { type: 'currency' }).of(code) ?? code : code
  } catch {
    return code
  }
}

// 获取所有支持的货币代码列表
export function getAllCurrencyCodes(): string[] {
  return [...new Set(EXCHANGE_RATE_API_CURRENCY_CODES)].sort((a, b) => a.localeCompare(b))
}

export function convertCost(value: number, currency: CurrencySettings): number {
  const rate = currency.exchangeRates[currency.displayCurrency] || 1.0
  return value * rate
}

export function formatCost(value: number, currency?: CurrencySettings, precision = 4): string {
  const rate = currency?.exchangeRates?.[currency.displayCurrency] ?? 1.0
  const converted = value * rate
  const symbol = currency ? getCurrencySymbol(currency.displayCurrency) : '$'
  return `${symbol}${Number.isFinite(converted) ? converted.toFixed(precision) : '0.0000'}`
}

export function formatRequestCount(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`
  if (value >= 1_000) return `${(value / 1_000).toFixed(2)}K`
  return String(Math.round(value))
}

export function formatTokenValue(value: number, unitBase?: number): string {
  const base = unitBase ?? value
  if (base >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`
  if (base >= 1_000) return `${(value / 1_000).toFixed(2)}K`
  return value.toFixed(2)
}

export function formatTokenPair(input: number, output: number): { input: string; output: string } {
  const base = Math.max(input, output)
  return {
    input: formatTokenValue(input, base),
    output: formatTokenValue(output, base)
  }
}

export function formatDurationMs(value: number): string {
  if (!value || value < 0) return '-'
  if (value < 1000) return `${value.toFixed(0)}ms`
  return `${(value / 1000).toFixed(2)}s`
}

export function formatRate(value: number): string {
  if (!value || value < 0) return '0'
  if (value >= 100) return value.toFixed(0)
  return value.toFixed(1)
}
