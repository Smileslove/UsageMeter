import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import ShareWindow from './ShareWindow.vue'
import DesktopApp from './apps/DesktopApp.vue'
import './styles.css'

const hash = window.location.hash
const Root = hash.startsWith('#/desktop') ? DesktopApp : hash.startsWith('#/share') ? ShareWindow : App

const app = createApp(Root)
app.config.errorHandler = (error, _instance, info) => {
  console.error('[Vue Error]', info, error)
}
app.config.warnHandler = (message, _instance, trace) => {
  console.warn('[Vue Warn]', message, trace)
}
app.use(createPinia())
app.mount('#app')
