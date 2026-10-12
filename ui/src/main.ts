import './app.css'
import { mount } from 'svelte'
import App from './App.svelte'
import { pwa } from './lib/pwa.svelte'

pwa.init()
const app = mount(App, { target: document.getElementById('app')! })
export default app
