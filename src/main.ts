import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import ShareWindow from './ShareWindow.vue'
import './styles.css'

const Root = window.location.hash.startsWith('#/share') ? ShareWindow : App

const app = createApp(Root)
app.use(createPinia())
app.mount('#app')
