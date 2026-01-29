# Справочник по Copernicus Data Space Ecosystem (CDSE) API

Этот документ содержит технические подробности о работе с OData и S3 API для проекта VeldMap.

## 1. Общие параметры
- **S3 Endpoint:** `https://eodata.dataspace.copernicus.eu`
- **S3 Region:** `cdse`
- **S3 Bucket:** `eodata`
- **OData URL:** `https://catalogue.dataspace.copernicus.eu/odata/v1/`

## 2. OData API

### Поиск продуктов
Основной эндпоинт для поиска: `.../odata/v1/Products`

### Ключевые атрибуты для поиска DEM:
Для фильтрации по атрибутам используется специфический синтаксис OData:
`$filter=Attributes/OData.CSC.StringAttribute/any(att:att/Name eq 'gridId' and att/OData.CSC.StringAttribute/Value eq 'N55_E037')`

- **`gridId`**: Индекс тайла в формате `<N/S><LAT>_<E/W><LON>`. Пример: `N55_E037`.
- **`productType`**: Тип данных. Для COG TIFF часто используется `SAR_DGE_30_A4AD` или префикс `COP-DEM`.
- **`dataset`**: Версия набора данных. Пример: `COP-DEM_GLO-30-DGED/2024_1`.
- **`eopIdentifier`**: Уникальный ID, часто содержащий путь и версию. Пример: `urn:eop:DLR:CDEM30:Copernicus_DSM_10_N55_00_E037_00:VORZ8-2019_2`.

### Ограничения:
- Лимит на `$skip`: не более 10,000.
- Функция `contains` **не поддерживается** внутри оператора `any` для атрибутов. Только точное совпадение `eq`.

## 3. S3 API (Хранилище eodata)

### Структура путей для Copernicus DEM (GLO-30 COG):
Файлы GeoTIFF (.tif) организованы в иерархию "Продукт -> Тайл -> Файл":

`auxdata/CopDEM/COP-DEM_GLO-30-DGED_PUBLIC/<PRODUCT_NAME>.DEM/<TILE_ID>/DEM/<TILE_ID>_DEM.tif`

**Пример реального пути:**
`auxdata/CopDEM/COP-DEM_GLO-30-DGED_PUBLIC/DEM1_SAR_DGE_30_20130601T090915_20140718T044012_ADS_000000_9763.DEM/Copernicus_DSM_10_S77_00_W106_00/DEM/Copernicus_DSM_10_S77_00_W106_00_DEM.tif`

### Компоненты пути:
1. **Коллекция**: `auxdata/CopDEM/COP-DEM_GLO-30-DGED_PUBLIC/` (публичные COG файлы).
2. **Имя продукта**: `DEM1_SAR_DGE_30_<DATE>_..._<HASH>.DEM`. Одна папка продукта может содержать множество тайлов для большого региона.
3. **ID Тайла**: `Copernicus_DSM_10_<LAT>_<LON>`. 
   - Формат координат в ID тайла: `N55_00_E037_00` (добавляются `_00` для точности).
4. **Файл**: Находится в подпапке `DEM/`.

## 4. Алгоритм быстрого поиска тайла
1. Запросить через OData список продуктов для нужного `gridId` (например, `N55_E037`).
2. Для каждого полученного `Name` продукта проверить существование пути на S3 в папке `..._PUBLIC/` с использованием сконструированного `TILE_ID`.
3. Если в `PUBLIC` не найдено, проверить основную папку `COP-DEM_GLO-30-DGED/`.

## 5. Аутентификация
Используется OpenID Connect (OAuth2). 
- **Token URL:** `https://identity.dataspace.copernicus.eu/auth/realms/CDSE/protocol/openid-connect/token`
- Требуются `username`, `password`, `client_id='cdse-public'`.
- Токен передается в заголовке: `Authorization: Bearer <TOKEN>`.
