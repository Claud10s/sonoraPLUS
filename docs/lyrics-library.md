# Biblioteca de letras y monitor

Abre el menú del proveedor de letras y elige **Biblioteca de letras y monitor de escuchas**.
También puedes pulsar **Ctrl+Shift+L** (Cmd+Shift+L en macOS).

El botón **ⓘ Cómo usar** abre y cierra las instrucciones de escuchas, edición y respaldos.
La guía está oculta al abrir el monitor para dejar más espacio a las canciones.

## Biblioteca local

La letra seleccionada se guarda en `state.sqlite`, separada de `cache.sqlite`. No tiene el
límite de 500 entradas de la caché. Al reproducir se consulta primero esta biblioteca y se
reutiliza su letra sin búsquedas de red. Las alternativas ya descargadas pueden aparecer en
el selector. Para consultar nuevas alternativas, abre el monitor y pulsa **Buscar en proveedores**.
La primera respuesta útil se conserva automáticamente; una búsqueda posterior nunca reemplaza
una letra guardada. Puedes abrir una respuesta en el editor y guardarla explícitamente.

Las letras se guardan por proveedor musical e identificador de publicación. Un título similar
no basta para compartir tiempos. **Usar como referencia** permite elegir otra publicación;
al abrir la canción de destino puedes vincular sus estadísticas, copiar solo el texto o copiar
el texto y sus tiempos. Las copias se pueden editar independientemente. No hay reconocimiento
automático mediante ISRC o audio en esta entrega.

El contador nuevo registra 30 segundos de audio reproducido, o la mitad de una pista de menos
de un minuto. Pausar y reanudar no añade otra escucha; adelantar la posición no cuenta como
tiempo escuchado. El historial anterior que todavía existe se incorpora una sola vez como
**inicios anteriores**, separado de las nuevas escuchas. No se reconstruyen registros eliminados
ni escuchas de otras aplicaciones. Los contadores y vínculos de canciones pertenecen a esta
instalación y no se suben a la nube.

## Archivos de letras

Sonora conserva automáticamente las respuestas válidas recibidas de los proveedores, incluidas
las alternativas, sin cambiar tu selección local. Se escriben en **Documentos/SonoraLetras**.
En **Carpeta de letras** puedes elegir otra ubicación o pulsar **Guardar archivos faltantes**.
Al iniciar se copian también las revisiones que ya estaban en la biblioteca; las alternativas
de la caché anterior se incorporan al volver a abrir la canción.

Cada letra tiene un TXT (plana) o LRC (sincronizada), y un JSON que conserva todos los detalles,
incluidos finales de palabras, traducciones y voces que LRC no representa completamente.
Los nombres incluyen artista, canción, proveedor de letra e identificador de contenido para
separar versiones y evitar duplicados. Nunca se sobrescriben los archivos existentes.
Las ediciones externas se usan importando el archivo en el editor; no se vigila la carpeta.
Si la carpeta no está disponible, se conserva la biblioteca interna y aparece un aviso.
La ruta es propia de esta instalación y no se incluye en el respaldo comprimido.

## Editor

1. Abre una canción y usa **Pegar letra** o importa un archivo UTF-8 TXT, LRC o JSON exportado
   por Sonora. Se admiten LRC por línea y LRC con palabras sincronizadas.
2. Reproduce esa publicación y pulsa **Marcar siguiente línea / palabra ahora** o **Ctrl+Enter**
   en el monitor al comenzar cada línea. En macOS puedes usar Cmd+Enter.
3. Ajusta los campos `mm:ss.mmm`. Los botones de desfase desplazan el borrador 0,1 segundos.
4. Para sincronización por palabra, pulsa **Preparar tiempos por palabra** en una línea y
   marca sus palabras sucesivamente. No se generan tiempos automáticamente.
5. Guarda. Los tiempos incompletos o desordenados no se aceptan. **Guardar como texto plano**
   permite guardar el texto sin tiempos.

Los cambios se guardan como nuevas revisiones. Puedes restaurar una revisión anterior sin
eliminar las demás. El editor avisa si intentas cambiar de canción o cerrar la ventana con un
borrador sin guardar. Espera a que termine el guardado antes de salir de toda la aplicación.
**Descartar borrador / cargar guardada** carga la letra después de importar o copiar una versión.

## Respaldo portátil, sin conexión de cuentas

Abre **Respaldos de letras** y pulsa **Crear respaldo comprimido**. Elige una carpeta;
Sonora genera `Sonora-Lyrics-<fecha Unix>-<identificador>.json.gz` con todas las letras,
metadatos de sus publicaciones, tiempos, fuentes, revisiones y selecciones actuales.
Cada respaldo tiene un nombre nuevo y no reemplaza archivos anteriores.

Puedes elegir una carpeta que gestione Google Drive o OneDrive, o subir el archivo desde
su página web. La transferencia la haces tú o el cliente de ese servicio. Sonora no
solicita permisos OAuth, no inicia sesiones en la nube y no realiza sincronización periódica.
La marca **En el último respaldo** indica que esa revisión está en el último archivo creado
o cargado; no confirma que el archivo se haya subido ni que siga existiendo en la nube.

Para recuperar letras, descarga el archivo desde Google Drive o OneDrive (o localízalo en
una carpeta sincronizada), pulsa **Cargar respaldo** y selecciónalo. El selector abre archivos
locales; no navega directamente por tus cuentas de nube. La carga incorpora las revisiones
faltantes, conserva las selecciones locales existentes y recupera las selecciones del respaldo
para canciones sin letra guardada. Cargar el mismo archivo otra vez no duplica revisiones.
Si hay diferencias, puedes revisar y elegir una versión desde el editor.

Los contadores de escuchas, vínculos entre publicaciones, carpetas de respaldo y credenciales no
se incluyen en el archivo. El respaldo conserva letras, no música. Guarda el borrador del
editor antes de crear un respaldo para que incluya tus últimos cambios.

## Estructura del archivo

Es un documento JSON UTF-8 comprimido con gzip, que se puede abrir con herramientas compatibles.
El formato 1 contiene:

- `format`: `sonora-lyrics`.
- `version`: `1`.
- `created_at`: fecha de creación en segundos Unix.
- `selected`: mapa de claves `proveedor:id` a identificadores de revisión seleccionados.
- `revisions`: lista de revisiones, cada una con `id`, `key`, `provider`, `track`, `source`,
  `sheet`, `parents` y `created`. `sheet` guarda texto plano o líneas y palabras con tiempos.

Los identificadores de revisión son SHA-256 de su contenido serializado con el campo `id`
vacío. La importación comprueba formato, versión, integridad, selecciones y referencias a
revisiones anteriores antes de guardar cambios. Un archivo inválido deja intacta la biblioteca.
Se aceptan hasta 50.000 revisiones y 256 MiB descomprimidos. No se extraen rutas ni archivos
contenidos en el respaldo. El archivo está comprimido, no cifrado.
