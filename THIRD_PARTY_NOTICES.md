# Сторонние компоненты

Videonarez распространяется по лицензии GNU GPL версии 3 или более поздней (см. `LICENSE`).
Вместе с установщиком поставляются следующие сторонние программы.

## FFmpeg

- Файлы: `ffmpeg.exe`, `ffprobe.exe` (запускаются как отдельные программы).
- Версия: 9.0.1, сборка «essentials» Гьяна Доши (gyan.dev).
- Лицензия: GNU GPL версии 3 или более поздней (сборка собрана с `--enable-gpl --enable-version3`).
  Полный текст лицензии — в файле `LICENSE`.
- Исходный код FFmpeg: https://ffmpeg.org/download.html и https://github.com/FFmpeg/FFmpeg
  (тег `n9.0.1`). Параметры сборки и список включённых библиотек: https://www.gyan.dev/ffmpeg/builds/
  и вывод команды `ffmpeg -buildconf`.
- Если исходный код именно этой сборки станет недоступен по ссылкам выше, его можно запросить
  у автора Videonarez через Issues репозитория; предложение действует три года с даты выпуска версии.

## Прочие библиотеки

Приложение собрано на Tauri, Svelte и библиотеках Rust и npm (лицензии MIT, Apache-2.0 и совместимые с GPL-3.0).
Полный список зависимостей — в `src-tauri/Cargo.lock` и `package-lock.json`.

Шумо- и эхоподавление микрофона: библиотека sonora (https://github.com/dignifiedquire/sonora), перенос
WebRTC Audio Processing на Rust. Лицензия BSD-3-Clause; Copyright (c) The WebRTC project authors и авторы sonora.

Шрифты Golos Text и Unbounded распространяются по лицензии SIL Open Font License 1.1.
