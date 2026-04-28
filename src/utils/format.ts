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
  ALL: 'L',
  ANG: 'ƒ',
  AOA: 'Kz',
  AWG: 'ƒ',
  BAM: 'KM',
  BBD: 'Bds$',
  BHD: 'BD',
  BIF: 'FBu',
  BMD: 'BD$',
  BOB: 'Bs',
  BSD: 'B$',
  BTN: 'Nu.',
  BWP: 'P',
  BYN: 'Br',
  BZD: 'BZ$',
  CDF: 'FC',
  CLF: 'UF',
  CNH: '¥',
  CRC: '₡',
  CUP: '$MN',
  CVE: 'Esc',
  DJF: 'Fdj',
  DOP: 'RD$',
  ERN: 'Nfk',
  FJD: 'FJ$',
  FKP: 'FK£',
  FOK: 'kr',
  GEL: '₾',
  GGP: '£',
  GIP: '£',
  GMD: 'D',
  GNF: 'FG',
  GTQ: 'Q',
  GYD: 'G$',
  HNL: 'L',
  HTG: 'G',
  IMP: '£',
  JEP: '£',
  JMD: 'J$',
  KGS: 'сом',
  KHR: '៛',
  KID: 'KID$',
  KMF: 'CF',
  KYD: 'CI$',
  KZT: '₸',
  LAK: '₭',
  LRD: 'L$',
  LSL: 'L',
  MDL: 'L',
  MGA: 'Ar',
  MKD: 'ден',
  MMK: 'K',
  MNT: '₮',
  MOP: 'MOP$',
  MRU: 'UM',
  MUR: '₨',
  MVR: 'Rf',
  MWK: 'MK',
  MZN: 'MT',
  NAD: 'N$',
  NIO: 'C$',
  PAB: 'B/.',
  PEN: 'S/',
  PGK: 'K',
  PYG: '₲',
  RSD: 'дин',
  RWF: 'RF',
  SBD: 'SI$',
  SCR: '₨',
  SDG: 'SDG',
  SHP: '£',
  SLE: 'Le',
  SLL: 'Le',
  SOS: 'Sh.So.',
  SRD: 'Sr$',
  SSP: 'SS£',
  STN: 'Db',
  SZL: 'E',
  TJS: 'SM',
  TMT: 'm',
  TOP: 'T$',
  TTD: 'TT$',
  TVD: 'TVD$',
  UYU: '$U',
  UZS: 'soʻm',
  VES: 'Bs.',
  VUV: 'VT',
  WST: 'WS$',
  XAF: 'FCFA',
  XCD: 'EC$',
  XCG: 'Cg',
  XDR: 'SDR',
  XOF: 'CFA',
  XPF: 'CFPF',
  ZWG: 'ZiG',
  ZWL: 'Z$',
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
  ALL: { 'zh-CN': '阿尔巴尼亚列克', 'zh-TW': '阿爾巴尼亞列克', 'en-US': 'Albanian Lek' },
  ANG: { 'zh-CN': '荷属安的列斯盾', 'zh-TW': '荷屬安地列斯盾', 'en-US': 'Netherlands Antillean Guilder' },
  AOA: { 'zh-CN': '安哥拉宽扎', 'zh-TW': '安哥拉寬扎', 'en-US': 'Angolan Kwanza' },
  AWG: { 'zh-CN': '阿鲁巴弗罗林', 'zh-TW': '阿魯巴弗羅林', 'en-US': 'Aruban Florin' },
  BAM: { 'zh-CN': '波黑可兑换马克', 'zh-TW': '波士尼亞與赫塞哥維納可兌換馬克', 'en-US': 'Bosnia-Herzegovina Convertible Mark' },
  BBD: { 'zh-CN': '巴巴多斯元', 'zh-TW': '巴貝多元', 'en-US': 'Barbadian Dollar' },
  BHD: { 'zh-CN': '巴林第纳尔', 'zh-TW': '巴林第納爾', 'en-US': 'Bahraini Dinar' },
  BIF: { 'zh-CN': '布隆迪法郎', 'zh-TW': '蒲隆地法郎', 'en-US': 'Burundian Franc' },
  BMD: { 'zh-CN': '百慕大元', 'zh-TW': '百慕達元', 'en-US': 'Bermudian Dollar' },
  BOB: { 'zh-CN': '玻利维亚诺', 'zh-TW': '玻利維亞諾', 'en-US': 'Bolivian Boliviano' },
  BSD: { 'zh-CN': '巴哈马元', 'zh-TW': '巴哈馬元', 'en-US': 'Bahamian Dollar' },
  BTN: { 'zh-CN': '不丹努尔特鲁姆', 'zh-TW': '不丹努爾特魯姆', 'en-US': 'Bhutanese Ngultrum' },
  BWP: { 'zh-CN': '博茨瓦纳普拉', 'zh-TW': '波札那普拉', 'en-US': 'Botswana Pula' },
  BYN: { 'zh-CN': '白俄罗斯卢布', 'zh-TW': '白俄羅斯盧布', 'en-US': 'Belarusian Ruble' },
  BZD: { 'zh-CN': '伯利兹元', 'zh-TW': '貝里斯元', 'en-US': 'Belize Dollar' },
  CDF: { 'zh-CN': '刚果法郎', 'zh-TW': '剛果法郎', 'en-US': 'Congolese Franc' },
  CLF: { 'zh-CN': '智利记账单位', 'zh-TW': '智利記帳單位', 'en-US': 'Chilean Unit of Account' },
  CNH: { 'zh-CN': '离岸人民币', 'zh-TW': '離岸人民幣', 'en-US': 'Offshore Chinese Yuan' },
  CRC: { 'zh-CN': '哥斯达黎加科朗', 'zh-TW': '哥斯大黎加科朗', 'en-US': 'Costa Rican Colon' },
  CUP: { 'zh-CN': '古巴比索', 'zh-TW': '古巴披索', 'en-US': 'Cuban Peso' },
  CVE: { 'zh-CN': '佛得角埃斯库多', 'zh-TW': '維德角埃斯庫多', 'en-US': 'Cape Verdean Escudo' },
  DJF: { 'zh-CN': '吉布提法郎', 'zh-TW': '吉布地法郎', 'en-US': 'Djiboutian Franc' },
  DOP: { 'zh-CN': '多米尼加比索', 'zh-TW': '多明尼加披索', 'en-US': 'Dominican Peso' },
  ERN: { 'zh-CN': '厄立特里亚纳克法', 'zh-TW': '厄利垂亞納克法', 'en-US': 'Eritrean Nakfa' },
  FJD: { 'zh-CN': '斐济元', 'zh-TW': '斐濟元', 'en-US': 'Fijian Dollar' },
  FKP: { 'zh-CN': '福克兰群岛镑', 'zh-TW': '福克蘭群島鎊', 'en-US': 'Falkland Islands Pound' },
  FOK: { 'zh-CN': '法罗群岛克朗', 'zh-TW': '法羅群島克朗', 'en-US': 'Faroese Krona' },
  GEL: { 'zh-CN': '格鲁吉亚拉里', 'zh-TW': '喬治亞拉里', 'en-US': 'Georgian Lari' },
  GGP: { 'zh-CN': '根西镑', 'zh-TW': '根西鎊', 'en-US': 'Guernsey Pound' },
  GIP: { 'zh-CN': '直布罗陀镑', 'zh-TW': '直布羅陀鎊', 'en-US': 'Gibraltar Pound' },
  GMD: { 'zh-CN': '冈比亚达拉西', 'zh-TW': '甘比亞達拉西', 'en-US': 'Gambian Dalasi' },
  GNF: { 'zh-CN': '几内亚法郎', 'zh-TW': '幾內亞法郎', 'en-US': 'Guinean Franc' },
  GTQ: { 'zh-CN': '危地马拉格查尔', 'zh-TW': '瓜地馬拉格查爾', 'en-US': 'Guatemalan Quetzal' },
  GYD: { 'zh-CN': '圭亚那元', 'zh-TW': '蓋亞那元', 'en-US': 'Guyanese Dollar' },
  HNL: { 'zh-CN': '洪都拉斯伦皮拉', 'zh-TW': '宏都拉斯倫皮拉', 'en-US': 'Honduran Lempira' },
  HTG: { 'zh-CN': '海地古德', 'zh-TW': '海地古德', 'en-US': 'Haitian Gourde' },
  IMP: { 'zh-CN': '马恩岛镑', 'zh-TW': '曼島鎊', 'en-US': 'Isle of Man Pound' },
  JEP: { 'zh-CN': '泽西镑', 'zh-TW': '澤西鎊', 'en-US': 'Jersey Pound' },
  JMD: { 'zh-CN': '牙买加元', 'zh-TW': '牙買加元', 'en-US': 'Jamaican Dollar' },
  KGS: { 'zh-CN': '吉尔吉斯斯坦索姆', 'zh-TW': '吉爾吉斯索姆', 'en-US': 'Kyrgyzstani Som' },
  KHR: { 'zh-CN': '柬埔寨瑞尔', 'zh-TW': '柬埔寨瑞爾', 'en-US': 'Cambodian Riel' },
  KID: { 'zh-CN': '基里巴斯元', 'zh-TW': '吉里巴斯元', 'en-US': 'Kiribati Dollar' },
  KMF: { 'zh-CN': '科摩罗法郎', 'zh-TW': '葛摩法郎', 'en-US': 'Comorian Franc' },
  KYD: { 'zh-CN': '开曼群岛元', 'zh-TW': '開曼群島元', 'en-US': 'Cayman Islands Dollar' },
  KZT: { 'zh-CN': '哈萨克斯坦坚戈', 'zh-TW': '哈薩克堅戈', 'en-US': 'Kazakhstani Tenge' },
  LAK: { 'zh-CN': '老挝基普', 'zh-TW': '寮國基普', 'en-US': 'Lao Kip' },
  LRD: { 'zh-CN': '利比里亚元', 'zh-TW': '賴比瑞亞元', 'en-US': 'Liberian Dollar' },
  LSL: { 'zh-CN': '莱索托洛蒂', 'zh-TW': '賴索托洛蒂', 'en-US': 'Lesotho Loti' },
  MDL: { 'zh-CN': '摩尔多瓦列伊', 'zh-TW': '摩爾多瓦列伊', 'en-US': 'Moldovan Leu' },
  MGA: { 'zh-CN': '马达加斯加阿里亚里', 'zh-TW': '馬達加斯加阿里亞里', 'en-US': 'Malagasy Ariary' },
  MKD: { 'zh-CN': '马其顿代纳尔', 'zh-TW': '馬其頓代納爾', 'en-US': 'Macedonian Denar' },
  MMK: { 'zh-CN': '缅甸元', 'zh-TW': '緬甸元', 'en-US': 'Myanmar Kyat' },
  MNT: { 'zh-CN': '蒙古图格里克', 'zh-TW': '蒙古圖格里克', 'en-US': 'Mongolian Tugrik' },
  MOP: { 'zh-CN': '澳门元', 'zh-TW': '澳門元', 'en-US': 'Macanese Pataca' },
  MRU: { 'zh-CN': '毛里塔尼亚乌吉亚', 'zh-TW': '茅利塔尼亞烏吉亞', 'en-US': 'Mauritanian Ouguiya' },
  MUR: { 'zh-CN': '毛里求斯卢比', 'zh-TW': '模里西斯盧比', 'en-US': 'Mauritian Rupee' },
  MVR: { 'zh-CN': '马尔代夫拉菲亚', 'zh-TW': '馬爾地夫拉菲亞', 'en-US': 'Maldivian Rufiyaa' },
  MWK: { 'zh-CN': '马拉维克瓦查', 'zh-TW': '馬拉威克瓦查', 'en-US': 'Malawian Kwacha' },
  MZN: { 'zh-CN': '莫桑比克梅蒂卡尔', 'zh-TW': '莫三比克梅蒂卡爾', 'en-US': 'Mozambican Metical' },
  NAD: { 'zh-CN': '纳米比亚元', 'zh-TW': '納米比亞元', 'en-US': 'Namibian Dollar' },
  NIO: { 'zh-CN': '尼加拉瓜科多巴', 'zh-TW': '尼加拉瓜科多巴', 'en-US': 'Nicaraguan Cordoba' },
  PAB: { 'zh-CN': '巴拿马巴波亚', 'zh-TW': '巴拿馬巴波亞', 'en-US': 'Panamanian Balboa' },
  PEN: { 'zh-CN': '秘鲁索尔', 'zh-TW': '秘魯索爾', 'en-US': 'Peruvian Sol' },
  PGK: { 'zh-CN': '巴布亚新几内亚基那', 'zh-TW': '巴布亞紐幾內亞基那', 'en-US': 'Papua New Guinean Kina' },
  PYG: { 'zh-CN': '巴拉圭瓜拉尼', 'zh-TW': '巴拉圭瓜拉尼', 'en-US': 'Paraguayan Guarani' },
  RSD: { 'zh-CN': '塞尔维亚第纳尔', 'zh-TW': '塞爾維亞第納爾', 'en-US': 'Serbian Dinar' },
  RWF: { 'zh-CN': '卢旺达法郎', 'zh-TW': '盧安達法郎', 'en-US': 'Rwandan Franc' },
  SBD: { 'zh-CN': '所罗门群岛元', 'zh-TW': '索羅門群島元', 'en-US': 'Solomon Islands Dollar' },
  SCR: { 'zh-CN': '塞舌尔卢比', 'zh-TW': '塞席爾盧比', 'en-US': 'Seychellois Rupee' },
  SDG: { 'zh-CN': '苏丹镑', 'zh-TW': '蘇丹鎊', 'en-US': 'Sudanese Pound' },
  SHP: { 'zh-CN': '圣赫勒拿镑', 'zh-TW': '聖赫勒拿鎊', 'en-US': 'Saint Helena Pound' },
  SLE: { 'zh-CN': '塞拉利昂利昂', 'zh-TW': '獅子山利昂', 'en-US': 'Sierra Leonean Leone' },
  SLL: { 'zh-CN': '塞拉利昂旧利昂', 'zh-TW': '獅子山舊利昂', 'en-US': 'Sierra Leonean Leone (Old)' },
  SOS: { 'zh-CN': '索马里先令', 'zh-TW': '索馬利亞先令', 'en-US': 'Somali Shilling' },
  SRD: { 'zh-CN': '苏里南元', 'zh-TW': '蘇利南元', 'en-US': 'Surinamese Dollar' },
  SSP: { 'zh-CN': '南苏丹镑', 'zh-TW': '南蘇丹鎊', 'en-US': 'South Sudanese Pound' },
  STN: { 'zh-CN': '圣多美和普林西比多布拉', 'zh-TW': '聖多美普林西比多布拉', 'en-US': 'Sao Tome and Principe Dobra' },
  SZL: { 'zh-CN': '斯威士兰里兰吉尼', 'zh-TW': '史瓦帝尼里蘭吉尼', 'en-US': 'Swazi Lilangeni' },
  TJS: { 'zh-CN': '塔吉克斯坦索莫尼', 'zh-TW': '塔吉克索莫尼', 'en-US': 'Tajikistani Somoni' },
  TMT: { 'zh-CN': '土库曼斯坦马纳特', 'zh-TW': '土庫曼馬納特', 'en-US': 'Turkmenistani Manat' },
  TOP: { 'zh-CN': '汤加潘加', 'zh-TW': '東加潘加', 'en-US': 'Tongan Paanga' },
  TTD: { 'zh-CN': '特立尼达和多巴哥元', 'zh-TW': '千里達及托巴哥元', 'en-US': 'Trinidad and Tobago Dollar' },
  TVD: { 'zh-CN': '图瓦卢元', 'zh-TW': '吐瓦魯元', 'en-US': 'Tuvaluan Dollar' },
  UYU: { 'zh-CN': '乌拉圭比索', 'zh-TW': '烏拉圭披索', 'en-US': 'Uruguayan Peso' },
  UZS: { 'zh-CN': '乌兹别克斯坦苏姆', 'zh-TW': '烏茲別克索姆', 'en-US': 'Uzbekistani Som' },
  VES: { 'zh-CN': '委内瑞拉玻利瓦尔', 'zh-TW': '委內瑞拉玻利瓦爾', 'en-US': 'Venezuelan Bolivar' },
  VUV: { 'zh-CN': '瓦努阿图瓦图', 'zh-TW': '萬那杜瓦圖', 'en-US': 'Vanuatu Vatu' },
  WST: { 'zh-CN': '萨摩亚塔拉', 'zh-TW': '薩摩亞塔拉', 'en-US': 'Samoan Tala' },
  XAF: { 'zh-CN': '中非法郎', 'zh-TW': '中非法郎', 'en-US': 'Central African CFA Franc' },
  XCD: { 'zh-CN': '东加勒比元', 'zh-TW': '東加勒比元', 'en-US': 'East Caribbean Dollar' },
  XCG: { 'zh-CN': '加勒比盾', 'zh-TW': '加勒比盾', 'en-US': 'Caribbean Guilder' },
  XDR: { 'zh-CN': '特别提款权', 'zh-TW': '特別提款權', 'en-US': 'Special Drawing Rights' },
  XOF: { 'zh-CN': '西非法郎', 'zh-TW': '西非法郎', 'en-US': 'West African CFA Franc' },
  XPF: { 'zh-CN': '太平洋法郎', 'zh-TW': '太平洋法郎', 'en-US': 'CFP Franc' },
  ZWG: { 'zh-CN': '津巴布韦金', 'zh-TW': '辛巴威金', 'en-US': 'Zimbabwe Gold' },
  ZWL: { 'zh-CN': '津巴布韦元', 'zh-TW': '辛巴威元', 'en-US': 'Zimbabwean Dollar' },
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
