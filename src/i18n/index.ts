import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./en.json";
import es from "./es.json";

const resources = { en: { translation: en }, es: { translation: es } };
const lang = navigator.language.slice(0, 2);

void i18n.use(initReactI18next).init({
  resources,
  lng: lang in resources ? lang : "en",
  fallbackLng: "en",
  interpolation: { escapeValue: false },
});

export default i18n;
