import i18n from 'i18next'
import { initReactI18next } from 'react-i18next'
import { en } from './en'
import { zhCN } from './zh-CN'

function initialLang(): 'zh-CN' | 'en' {
  try {
    const raw = localStorage.getItem('imagepicker.ui')
    const lang = raw ? (JSON.parse(raw) as { state?: { lang?: string } }).state?.lang : undefined
    if (lang === 'en' || lang === 'zh-CN') return lang
  } catch {
    /* ignore */
  }
  return 'zh-CN'
}

void i18n.use(initReactI18next).init({
  resources: { 'zh-CN': { translation: zhCN }, en: { translation: en } },
  lng: initialLang(),
  fallbackLng: 'zh-CN',
  interpolation: { escapeValue: false },
})

export default i18n
