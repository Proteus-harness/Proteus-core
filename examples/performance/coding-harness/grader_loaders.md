# Узкая проверка загрузчиков grader

```sh
python3 -B examples/performance/coding-harness/grader_loaders_test.py -v
```

Это stdlib loader-only controls с собственными синтетическими Python-модулями,
не запуск полного pytest grader, CLI archive или реального benchmark. Декоратор
pytest заменён только для загрузки файла с объявлениями тестов. Journal grader
использует один локальный helper для archive и measurements: регистрация в
`sys.modules` до исполнения допускает обычные dataclasses с postponed annotations.
Success оставляет свежий модуль под служебным именем; при `BaseException` прежняя
запись (включая `None`) восстанавливается, а при её отсутствии новая удаляется.
Повторная успешная загрузка остаётся свежей, без изменения старого module object.

Остальные grading assertions и задача не меняются. Исправление loader не
переоценивает сохранённые результаты: пересмотр требует исходного workspace,
подробного traceback и отдельно зафиксированной версии grader. Эти controls
не подтверждают корректность archive, полную совместимость grader или изменение
каких-либо опубликованных benchmark scores.
