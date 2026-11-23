//! Translations: English (the text in the code), Spanish and Catalan.
//!
//! Every text the person sees goes through `t("English text")`, which returns the text in the
//! chosen language (Settings > Language; "Automatic" follows Windows' display language). Texts
//! with values use `{}` placeholders filled in order by `tf`. A text missing from a table simply
//! shows in English. Month and day names come from `month_name` / `weekday_name`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Es,
    Ca,
}

/// 0 = English, 1 = Spanish, 2 = Catalan (the language in use now).
static LANG: AtomicU8 = AtomicU8::new(0);

/// The choices in Settings, in order: (setting value, name shown in its own language).
pub const CHOICES: &[(u8, &str)] = &[(0, "Automatic (Windows)"), (1, "English"), (2, "Español"), (3, "Català")];

/// Windows' display language for this user, as one of ours (English if neither).
pub fn windows_lang() -> Lang {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Globalization::GetUserDefaultUILanguage;
        // The low 10 bits are the primary language: 0x0A Spanish, 0x03 Catalan.
        match GetUserDefaultUILanguage() & 0x3FF {
            0x0A => return Lang::Es,
            0x03 => return Lang::Ca,
            _ => {}
        }
    }
    Lang::En
}

/// Applies the setting (0 = automatic, 1 = English, 2 = Spanish, 3 = Catalan).
pub fn set_lang(setting: u8) {
    let l = match setting {
        1 => Lang::En,
        2 => Lang::Es,
        3 => Lang::Ca,
        _ => windows_lang(),
    };
    LANG.store(
        match l {
            Lang::En => 0,
            Lang::Es => 1,
            Lang::Ca => 2,
        },
        Ordering::Relaxed,
    );
}

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Es,
        2 => Lang::Ca,
        _ => Lang::En,
    }
}

fn table(l: Lang) -> &'static HashMap<&'static str, &'static str> {
    static ES_MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    static CA_MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    static EMPTY: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    match l {
        Lang::Es => ES_MAP.get_or_init(|| ES.iter().copied().collect()),
        Lang::Ca => CA_MAP.get_or_init(|| CA.iter().copied().collect()),
        Lang::En => EMPTY.get_or_init(HashMap::new),
    }
}

/// The text in the language in use (the English text if it has no translation).
pub fn t(s: &'static str) -> &'static str {
    match lang() {
        Lang::En => s,
        l => table(l).get(s).copied().unwrap_or(s),
    }
}

/// `t` with `{}` placeholders filled in order: tf("{} items", &[&n]).
pub fn tf(s: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::new();
    let mut rest = t(s);
    for a in args {
        match rest.find("{}") {
            Some(i) => {
                out.push_str(&rest[..i]);
                out.push_str(&a.to_string());
                rest = &rest[i + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Full month name (1 = January).
pub fn month_name(m: u32) -> &'static str {
    const EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    const ES_M: [&str; 12] = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];
    const CA_M: [&str; 12] = ["gener", "febrer", "març", "abril", "maig", "juny", "juliol", "agost", "setembre", "octubre", "novembre", "desembre"];
    let i = (m.clamp(1, 12) - 1) as usize;
    match lang() {
        Lang::En => EN[i],
        Lang::Es => ES_M[i],
        Lang::Ca => CA_M[i],
    }
}

/// Short month name (1 = Jan).
pub fn month_short(m: u32) -> &'static str {
    const EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const ES_M: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"];
    const CA_M: [&str; 12] = ["gen", "febr", "març", "abr", "maig", "juny", "jul", "ag", "set", "oct", "nov", "des"];
    let i = (m.clamp(1, 12) - 1) as usize;
    match lang() {
        Lang::En => EN[i],
        Lang::Es => ES_M[i],
        Lang::Ca => CA_M[i],
    }
}

/// Full day name (0 = Monday).
pub fn weekday_name(d: u32) -> &'static str {
    const EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    const ES_D: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
    const CA_D: [&str; 7] = ["dilluns", "dimarts", "dimecres", "dijous", "divendres", "dissabte", "diumenge"];
    let i = (d % 7) as usize;
    match lang() {
        Lang::En => EN[i],
        Lang::Es => ES_D[i],
        Lang::Ca => CA_D[i],
    }
}

/// Two-letter day name (0 = Monday): Mo / Lu / Dl.
pub fn weekday_short(d: u32) -> &'static str {
    const EN: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
    const ES_D: [&str; 7] = ["Lu", "Ma", "Mi", "Ju", "Vi", "Sá", "Do"];
    const CA_D: [&str; 7] = ["Dl", "Dt", "Dc", "Dj", "Dv", "Ds", "Dg"];
    let i = (d % 7) as usize;
    match lang() {
        Lang::En => EN[i],
        Lang::Es => ES_D[i],
        Lang::Ca => CA_D[i],
    }
}

/// "Sunday, 22 November" / "domingo, 22 de noviembre" / "diumenge, 22 de novembre".
pub fn long_date(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let wd = weekday_name(d.weekday().num_days_from_monday());
    format!("{wd}, {}", day_month(d))
}

/// "22 November" / "22 de noviembre" / "22 de novembre" (no weekday).
pub fn day_month(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let m = month_name(d.month());
    match lang() {
        Lang::En => format!("{} {m}", d.day()),
        Lang::Es | Lang::Ca => {
            let de = if lang() == Lang::Ca && matches!(m.chars().next(), Some('a' | 'o')) { "d'" } else { "de " };
            format!("{} {de}{m}", d.day())
        }
    }
}

/// "November 2026" / "noviembre de 2026" / "novembre de 2026".
pub fn month_year(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let m = month_name(d.month());
    match lang() {
        Lang::En => format!("{m} {}", d.year()),
        _ => format!("{m} de {}", d.year()),
    }
}

/// "22 Nov" / "22 nov".
pub fn short_date(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    format!("{} {}", d.day(), month_short(d.month()))
}

// ---- tables: (English text exactly as in the code, translation) -------------------------------
// Keep each table in the same order as the other; a missing entry shows in English.

pub static ES: &[(&str, &str)] = &[
    ("Automatic (Windows)", "Automático (Windows)"),
    ("that link isn't an iCal calendar (.ics)", "ese enlace no es un calendario iCal (.ics)"),
    ("that calendar has something DesktopShepherd can't read", "ese calendario contiene algo que DesktopShepherd no puede leer"),
    ("the link must start with https://, http:// or webcal://", "el enlace debe empezar por https://, http:// o webcal://"),
    ("the link has no server name", "el enlace no tiene nombre de servidor"),
    ("Windows' web access could not start", "no se pudo iniciar el acceso web de Windows"),
    ("could not reach {}", "no se pudo conectar con {}"),
    ("could not prepare the request", "no se pudo preparar la solicitud"),
    ("could not reach {} ({})", "no se pudo conectar con {} ({})"),
    ("no answer from {} ({})", "{} no responde ({})"),
    ("{} refused the link (error {}): is it the private/secret iCal address?", "{} rechazó el enlace (error {}): ¿es la dirección iCal privada/secreta?"),
    ("that is the calendar's public address, which only works for public calendars: use its \"Secret address in iCal format\" instead", "esa es la dirección pública del calendario, que solo funciona con calendarios públicos: usa su \"Dirección secreta en formato iCal\""),
    ("{} doesn't know that link (error 404)", "{} no reconoce ese enlace (error 404)"),
    ("{} answered with error {}", "{} respondió con el error {}"),
    ("the download was interrupted", "la descarga se interrumpió"),
    ("that calendar is too big (over 8 MB)", "ese calendario es demasiado grande (más de 8 MB)"),
    ("not available on this system", "no disponible en este sistema"),
    ("Desktop", "Escritorio"),
    ("New box", "Nueva caja"),
    ("Export layout", "Exportar disposición"),
    ("Import layout", "Importar disposición"),
    ("Remove from this box", "Quitar de esta caja"),
    ("Open file location", "Abrir ubicación del archivo"),
    ("Name", "Nombre"),
    ("Unknown widget \"{}\"", "Widget desconocido \"{}\""),
    ("{} items", "{} elementos"),
    ("Everything on the desktop is in a box.", "Todo lo del escritorio está en una caja."),
    ("Drag files here, or right-click an icon › Move to.", "Arrastra archivos aquí, o haz clic derecho en un icono › Mover a."),
    ("Colour", "Color"),
    ("Any colour…", "Cualquier color…"),
    ("Opacity", "Opacidad"),
    ("Corners", "Esquinas"),
    ("Blur", "Desenfoque"),
    ("Border", "Borde"),
    ("Thickness", "Grosor"),
    ("Border colour…", "Color del borde…"),
    ("Text", "Texto"),
    ("Text colour…", "Color del texto…"),
    ("Shadow behind the text", "Sombra detrás del texto"),
    ("Drop shadow", "Sombra"),
    ("Title size", "Tamaño del título"),
    ("Text size", "Tamaño del texto"),
    ("Font", "Fuente"),
    ("Icons", "Iconos"),
    ("Show names under the icons", "Mostrar nombres bajo los iconos"),
    ("size", "tamaño"),
    ("auto", "auto"),
    ("Spacing", "Espaciado"),
    ("Title", "Título"),
    ("Off", "Desactivado"),
    ("Left", "Izquierda"),
    ("Top", "Arriba"),
    ("Bottom", "Abajo"),
    ("Tabs", "Pestañas"),
    ("Right side", "Lado derecho"),
    ("Apply this look to all boxes", "Aplicar este aspecto a todas las cajas"),
    ("Rename box", "Cambiar nombre de caja"),
    ("Roll up", "Enrollar"),
    ("Unroll", "Desenrollar"),
    ("Lock position and size", "Bloquear posición y tamaño"),
    ("View", "Vista"),
    ("Sort by", "Ordenar por"),
    ("New tab", "Nueva pestaña"),
    ("Tab {}", "Pestaña {}"),
    ("Collect other desktop items", "Reunir el resto de elementos del escritorio"),
    ("Shows every desktop item no other box holds, sorted into tabs by kind", "Muestra todos los elementos del escritorio que no están en otra caja, clasificados en pestañas por tipo"),
    ("Personalize…", "Personalizar…"),
    ("Widget settings…", "Configuración del widget…"),
    ("New", "Nuevo"),
    ("Move to page", "Mover a la página"),
    ("Keyboard shortcut", "Atajo de teclado"),
    ("Search desktop", "Buscar en el escritorio"),
    ("DesktopShepherd settings", "Configuración de DesktopShepherd"),
    ("Click again to delete (files stay)", "Haz clic de nuevo para eliminar (los archivos se conservan)"),
    ("Delete box", "Eliminar caja"),
    ("Small icons", "Iconos pequeños"),
    ("Medium icons", "Iconos medianos"),
    ("Large icons", "Iconos grandes"),
    ("Scroll when icons don't fit", "Desplazar cuando los iconos no caben"),
    ("Pages with dots when icons don't fit", "Páginas con puntos cuando los iconos no caben"),
    ("As I arrange them", "Como yo los organice"),
    ("Item type", "Tipo de elemento"),
    ("Date modified", "Fecha de modificación"),
    ("Times opened", "Veces abierto"),
    ("None", "Ninguno"),
    ("Brings the box into view; pressed again, rolls it up or down.", "Muestra la caja; si se pulsa de nuevo, la enrolla o desenrolla."),
    ("Page {} (shown now)", "Página {} (visible ahora)"),
    ("Page {}", "Página {}"),
    ("New page", "Nueva página"),
    ("Switch pages with Ctrl+Alt+PageDown / PageUp or the tray icon.", "Cambia de página con Ctrl+Alt+PageDown / PageUp o con el icono de la bandeja."),
    ("Draw a new box…", "Dibujar una caja nueva…"),
    ("Drag a rectangle anywhere on the screen (Ctrl+Alt+B)", "Arrastra un rectángulo en cualquier lugar de la pantalla (Ctrl+Alt+B)"),
    ("Box", "Caja"),
    ("Folder box…", "Caja de carpeta…"),
    ("A box that shows what is in a folder, live", "Una caja que muestra el contenido de una carpeta en tiempo real"),
    ("{} widget", "Widget {}"),
    ("Rename tab", "Cambiar nombre de pestaña"),
    ("Delete tab", "Eliminar pestaña"),
    ("Its items go back to the desktop", "Sus elementos vuelven al escritorio"),
    ("Move to", "Mover a"),
    ("Open", "Abrir"),
    ("Boxes", "Cajas"),
    ("Layout", "Disposición"),
    ("Settings", "Configuración"),
    ("➕ New box", "➕ Nueva caja"),
    ("➕ New auto-sort box", "➕ Nueva caja de clasificación automática"),
    ("Collects every desktop item no other box holds, sorted into tabs by kind", "Reúne todos los elementos del escritorio que no están en otra caja, clasificados en pestañas por tipo"),
    ("➕ New folder box…", "➕ Nueva caja de carpeta…"),
    ("➕ Add widget  {}", "➕ Añadir widget  {}"),
    ("Recently deleted:", "Eliminadas recientemente:"),
    ("Bring this box back", "Recuperar esta caja"),
    ("{} items on the desktop, {} not in a box yet. Tip: right-click a box for its options; double-click its title to roll it up.", "{} elementos en el escritorio, {} aún sin caja. Consejo: haz clic derecho en una caja para ver sus opciones; haz doble clic en su título para enrollarla."),
    ("Kind", "Tipo"),
    ("Contents", "Contenido"),
    ("State", "Estado"),
    ("Auto-sort box", "Caja de clasificación automática"),
    ("{} items in {} tab(s)", "{} elementos en {} pestaña(s)"),
    ("Rolled up", "Enrollada"),
    ("Locked", "Bloqueada"),
    ("Confirm delete", "Confirmar eliminación"),
    ("Delete", "Eliminar"),
    ("Main monitor", "Monitor principal"),
    ("Second monitor", "Segundo monitor"),
    ("Third monitor", "Tercer monitor"),
    ("Monitor {}", "Monitor {}"),
    ("{} (fixed){}  ·  {}", "{} (fijo){}  ·  {}"),
    (", main now", ", principal ahora"),
    ("All {} monitors", "Los {} monitores"),
    ("Your desktop, as the boxes sit on it", "Tu escritorio, con las cajas donde están"),
    ("Export layout…", "Exportar disposición…"),
    ("Save all boxes and settings to a file (it includes your agenda links)", "Guarda todas las cajas y la configuración en un archivo (incluye tus enlaces de agenda)"),
    ("Import layout…", "Importar disposición…"),
    ("Replace the boxes and settings with a saved file (the current ones are backed up first)", "Reemplaza las cajas y la configuración con un archivo guardado (antes se hace copia de las actuales)"),
    ("Bring every box back on screen", "Traer todas las cajas de vuelta a la pantalla"),
    ("Boxes stay on their display (Settings > Monitors: fixed).", "Las cajas permanecen en su pantalla (Configuración > Monitores: fijo)."),
    ("Boxes follow the monitor's role: main, second… (Settings > Monitors: by role).", "Las cajas siguen la función del monitor: principal, segundo… (Configuración > Monitores: por función)."),
    ("No boxes on this monitor yet", "Aún no hay cajas en este monitor"),
    ("All monitors", "Todos los monitores"),
    ("Language", "Idioma"),
    ("Add \"DesktopShepherd\" to the desktop's right-click menu (new box or widget where you click)", "Añadir \"DesktopShepherd\" al menú contextual del escritorio (caja o widget nuevo donde hagas clic)"),
    ("On Windows 11 it is under \"Show more options\". Or press Ctrl+Alt+B anywhere and drag a rectangle.", "En Windows 11 está en \"Mostrar más opciones\". O pulsa Ctrl+Alt+B en cualquier lugar y arrastra un rectángulo."),
    ("Hide Windows' own desktop icons (the boxes show them)", "Ocultar los iconos propios del escritorio de Windows (las cajas los muestran)"),
    ("They always come back when DesktopShepherd closes, even after a crash.", "Siempre vuelven al cerrar DesktopShepherd, incluso tras un fallo."),
    ("Open rolled-up boxes while the mouse is over them", "Abrir las cajas enrolladas mientras el ratón está encima"),
    ("Fade boxes while the mouse is away", "Atenuar las cajas mientras el ratón no está encima"),
    ("to", "a"),
    ("after", "tras"),
    ("Snap boxes to each other and to the screen edges", "Ajustar las cajas entre sí y a los bordes de la pantalla"),
    ("gap", "separación"),
    ("Also snap to a grid", "Ajustar también a una cuadrícula"),
    ("Hold Alt while moving or resizing a box to place it freely.", "Mantén pulsada Alt al mover o cambiar el tamaño de una caja para colocarla libremente."),
    ("Quick search also finds programs (from the Start menu)", "La búsqueda rápida también encuentra programas (del menú Inicio)"),
    ("Quick search shortcut:", "Atajo de teclado de búsqueda rápida:"),
    ("That shortcut is already used by another program: pick another one.", "Ese atajo ya lo usa otro programa: elige otro."),
    ("Hide / show all boxes:", "Ocultar / mostrar todas las cajas:"),
    ("Typing on the desktop:", "Escribir en el escritorio:"),
    ("Does nothing", "No hace nada"),
    ("Jumps to the icon that starts with it", "Salta al icono que empieza por ello"),
    ("Starts a quick search", "Inicia una búsqueda rápida"),
    ("Click an empty part of the desktop, then type: the same letter again goes to the next match, Enter opens it.", "Haz clic en una parte vacía del escritorio y escribe: la misma letra otra vez va a la siguiente coincidencia; Intro la abre."),
    ("Frosted glass: set each box's blur in its menu (Personalize > Blur).", "Cristal esmerilado: ajusta el desenfoque de cada caja en su menú (Personalizar > Desenfoque)."),
    ("Monitors", "Monitores"),
    ("Each Windows virtual desktop shows its own page of boxes (desktop 1 = page 1, desktop 2 = page 2…)", "Cada escritorio virtual de Windows muestra su propia página de cajas (escritorio 1 = página 1, escritorio 2 = página 2…)"),
    ("Remember box positions for each monitor setup (e.g. laptop alone / docked)", "Recordar las posiciones de las cajas para cada configuración de monitores (p. ej., portátil solo / con base)"),
    ("Forget", "Olvidar"),
    ("Bind each box to:", "Vincular cada caja a:"),
    ("The monitor's role (main, second…): when the roles change, the boxes move with them", "La función del monitor (principal, segundo…): cuando cambian las funciones, las cajas se mueven con ellas"),
    ("The monitor itself: boxes always stay on the monitor where they were created or moved", "El propio monitor: las cajas siempre permanecen en el monitor donde se crearon o movieron"),
    ("Moving a box to another monitor binds it there. Settings > Layout shows each monitor.", "Al mover una caja a otro monitor, queda vinculada allí. Configuración > Disposición muestra cada monitor."),
    ("{}: {} × {} ({}) — {} box(es)", "{}: {} × {} ({}) — {} caja(s)"),
    ("Auto-sort tabs", "Pestañas de clasificación automática"),
    ("Each tab collects items by file type, name pattern (* and ?) and/or how recently they changed; the first matching rule wins. Folders and everything else get their own tabs.", "Cada pestaña reúne elementos por tipo de archivo, patrón de nombre (* y ?) y/o por lo recientemente que cambiaron; gana la primera regla que coincida. Las carpetas y todo lo demás tienen sus propias pestañas."),
    ("types: zip, rar, 7z", "tipos: zip, rar, 7z"),
    ("name: *invoice*", "nombre: *factura*"),
    ("changed in last", "cambiado en los últimos"),
    (" days", " días"),
    ("0 = any age", "0 = cualquier antigüedad"),
    ("Remove", "Quitar"),
    ("Add tab rule", "Añadir regla de pestaña"),
    ("Reset to defaults", "Restablecer valores predeterminados"),
    ("Computer", "Equipo"),
    ("Start DesktopShepherd when Windows starts", "Iniciar DesktopShepherd al iniciar Windows"),
    ("Run at low priority (never slows other programs)", "Ejecutar con prioridad baja (nunca ralentiza otros programas)"),
    ("New, renamed and removed desktop files show up as soon as Windows reports them.", "Los archivos del escritorio nuevos, renombrados y eliminados aparecen en cuanto Windows los notifica."),
    ("Saved layouts", "Disposiciones guardadas"),
    ("A copy of your layout is kept each time DesktopShepherd starts (the last 10).", "Se guarda una copia de tu disposición cada vez que se inicia DesktopShepherd (las últimas 10)."),
    ("Restore", "Restaurar"),
    ("Open data folder", "Abrir carpeta de datos"),
    ("Quit DesktopShepherd", "Salir de DesktopShepherd"),
    ("{} desktop items", "{} elementos del escritorio"),
    ("Apps", "Aplicaciones"),
    ("Clock", "Reloj"),
    ("Note", "Nota"),
    ("Welcome to DesktopShepherd!\n• Drag a box by its title, resize from its edges.\n• Double-click a title to roll the box up.\n• Right-click a box or an icon for options.", "¡Te damos la bienvenida a DesktopShepherd!\n• Arrastra una caja por su título y cambia su tamaño desde los bordes.\n• Haz doble clic en un título para enrollar la caja.\n• Haz clic derecho en una caja o un icono para ver las opciones."),
    ("Open DesktopShepherd", "Abrir DesktopShepherd"),
    ("Hide / show boxes", "Ocultar / mostrar cajas"),
    ("Next page of boxes", "Página siguiente de cajas"),
    ("Draw a new box…  (Ctrl+Alt+B)", "Dibujar una caja nueva…  (Ctrl+Alt+B)"),
    ("Bring back the last deleted box", "Recuperar la última caja eliminada"),
    ("Quit", "Salir"),
    ("Search the desktop…", "Buscar en el escritorio…"),
    ("Type part of a name. Enter opens, Ctrl+Enter opens its folder, Esc closes.", "Escribe parte de un nombre. Intro abre, Ctrl+Intro abre su carpeta, Esc cierra."),
    ("Nothing on the desktop matches.", "Nada en el escritorio coincide."),
    ("Program", "Programa"),
    ("Rename", "Cambiar nombre"),
    ("Enter renames, Esc cancels.", "Intro cambia el nombre, Esc cancela."),
    ("{}  Search the desktop", "{}  Buscar en el escritorio"),
    ("Nothing found", "No se encontró nada"),
    ("Personalize", "Personalizar"),
    ("Look", "Aspecto"),
    ("Widget settings", "Configuración del widget"),
    ("Done", "Listo"),
    ("“{}” deleted", "“{}” eliminada"),
    ("Undo", "Deshacer"),
    ("Choose a folder for the box", "Elige una carpeta para la caja"),
    ("Choose a picture", "Elige una imagen"),
    ("Pictures (PNG, JPEG)", "Imágenes (PNG, JPEG)"),
    ("New box here", "Nueva caja aquí"),
    ("New folder box here…", "Nueva caja de carpeta aquí…"),
    ("New widget here", "Nuevo widget aquí"),
    ("DesktopShepherd layout (*.json)", "Disposición de DesktopShepherd (*.json)"),
    ("Drag to draw the new box  ·  Esc to cancel", "Arrastra para dibujar la caja nueva  ·  Esc para cancelar"),
    ("Widget", "Widget"),
    ("Update {}?\n\nThe installed copy in\n{}\nwill be replaced. Your boxes stay as they are.", "¿Actualizar {}?\n\nLa copia instalada en\n{}\nse reemplazará. Tus cajas se quedan como están."),
    ("Install {}?\n\nIt goes to\n{}\n(just for you, no admin rights needed), gets a Start menu entry and starts with Windows.\n\nYou can remove it later from Settings > Apps > Installed apps.", "¿Instalar {}?\n\nSe instalará en\n{}\n(solo para ti, sin permisos de administrador), tendrá una entrada en el menú Inicio y se iniciará con Windows.\n\nPuedes quitarlo más tarde desde Configuración > Aplicaciones > Aplicaciones instaladas."),
    ("{} is still running and didn't close. Quit it from its tray icon, then run the setup again.", "{} sigue en ejecución y no se cerró. Sal desde su icono de la bandeja y vuelve a ejecutar la instalación."),
    ("Could not copy the program to\n{}\n\n{}", "No se pudo copiar el programa en\n{}\n\n{}"),
    ("{} is updated. It starts when you press OK; its icon is in the tray (next to the clock).", "{} se ha actualizado. Se inicia al pulsar Aceptar; su icono está en la bandeja (junto al reloj)."),
    ("{} is installed. It starts when you press OK; its icon is in the tray (next to the clock).", "{} se ha instalado. Se inicia al pulsar Aceptar; su icono está en la bandeja (junto al reloj)."),
    ("{} is installed but did not start: {}", "{} se ha instalado pero no se inició: {}"),
    ("Remove {} from this computer?", "¿Quitar {} de este equipo?"),
    ("{} is still running and didn't close. Quit it from its tray icon, then try again.", "{} sigue en ejecución y no se cerró. Sal desde su icono de la bandeja e inténtalo de nuevo."),
    ("Also delete your saved boxes and settings?\n\n{}\n\nChoose No to keep them in case you install {} again.", "¿Eliminar también tus cajas y configuración guardadas?\n\n{}\n\nElige No para conservarlas por si vuelves a instalar {}."),
    ("{} has been removed.", "{} se ha quitado."),
    ("Work", "Trabajo"),
    ("Break", "Descanso"),
    ("Time's up!", "¡Se acabó el tiempo!"),
    ("Timer", "Temporizador"),
    ("Pause", "Pausa"),
    ("Resume", "Reanudar"),
    ("Start", "Iniciar"),
    ("Reset", "Restablecer"),
    ("{} min", "{} min"),
    ("Pomodoro: 25 minutes of work, 5 of rest, a longer rest every fourth time", "Pomodoro: 25 minutos de trabajo, 5 de descanso, y un descanso más largo cada cuarta vez"),
    ("Countdown length", "Duración de la cuenta atrás"),
    ("Downloading the update…", "Descargando la actualización…"),
    ("Could not save the update", "No se pudo guardar la actualización"),
    ("Could not start the update", "No se pudo iniciar la actualización"),
    ("The downloaded file is not a program; opening the release page instead.", "El archivo descargado no es un programa; se abre la página de la versión en su lugar."),
    ("Could not download the update", "No se pudo descargar la actualización"),
    ("Clear", "Despejado"),
    ("Partly cloudy", "Parcialmente nublado"),
    ("Cloudy", "Nublado"),
    ("Fog", "Niebla"),
    ("Drizzle", "Llovizna"),
    ("Rain", "Lluvia"),
    ("Snow", "Nieve"),
    ("Thunderstorm", "Tormenta"),
    ("Right-click here and choose \"Widget settings…\" to pick a place.", "Haz clic derecho aquí y elige \"Configuración del widget…\" para elegir un lugar."),
    ("Loading…", "Cargando…"),
    ("Today", "Hoy"),
    ("Place", "Lugar"),
    ("Type a town or city", "Escribe una localidad o ciudad"),
    ("Search", "Buscar"),
    ("Nothing found for \"{}\".", "No se encontró nada para \"{}\"."),
    ("Weather data by Open-Meteo.com, updated every 30 minutes while the widget is shown.", "Datos meteorológicos de Open-Meteo.com, actualizados cada 30 minutos mientras se muestra el widget."),
    ("24-hour", "24 horas"),
    ("24-hour clock", "Reloj de 24 horas"),
    ("Type a note…", "Escribe una nota…"),
    ("{} GB free", "{} GB libres"),
    ("Back to today", "Volver a hoy"),
    ("Week starts on Sunday", "La semana empieza en domingo"),
    ("Add a task, then Enter", "Añade una tarea y pulsa Intro"),
    ("Remove finished tasks", "Quitar las tareas terminadas"),
    ("{} left", "{} pendientes"),
    ("Right-click here and choose \"Widget settings…\" to add a calendar link (iCal / .ics).", "Haz clic derecho aquí y elige \"Configuración del widget…\" para añadir un enlace de calendario (iCal / .ics)."),
    ("Google Calendar: Settings > your calendar > \"Secret address in iCal format\". Outlook: Settings > Calendar > Shared calendars > Publish.", "Google Calendar: Configuración > tu calendario > \"Dirección secreta en formato iCal\". Outlook: Configuración > Calendario > Calendarios compartidos > Publicar."),
    ("⚠ {} calendar(s) not updated: {}", "⚠ {} calendario(s) sin actualizar: {}"),
    ("Month", "Mes"),
    ("Week", "Semana"),
    ("Agenda", "Agenda"),
    ("Calendar links (iCal / .ics)", "Enlaces de calendario (iCal / .ics)"),
    ("The full link stays on this PC only: in your layout file and its backups.", "El enlace completo solo se queda en este PC: en tu archivo de disposición y sus copias de seguridad."),
    ("Paste a link: https://… or webcal://…", "Pega un enlace: https://… o webcal://…"),
    ("Add", "Añadir"),
    ("Show the next", "Mostrar los próximos"),
    ("{} days", "{} días"),
    ("Refresh now", "Actualizar ahora"),
    ("Calendars are downloaded every 30 minutes while this widget is shown.", "Los calendarios se descargan cada 30 minutos mientras se muestra este widget."),
    ("can't open the picture ({})", "no se puede abrir la imagen ({})"),
    ("can't read that picture", "no se puede leer esa imagen"),
    ("Right-click here and choose \"Widget settings…\" to pick a picture.", "Haz clic derecho aquí y elige \"Configuración del widget…\" para elegir una imagen."),
    ("Picture (PNG or JPEG)", "Imagen (PNG o JPEG)"),
    ("Choose…", "Elegir…"),
    ("Show the whole picture", "Mostrar la imagen completa"),
    ("Fill the box (edges may be cut)", "Rellenar la caja (los bordes pueden recortarse)"),
    ("Today, {}", "Hoy, {}"),
    ("Tomorrow, {}", "Mañana, {}"),
    ("Nothing in the next {} days.", "Nada en los próximos {} días."),
    ("Tomorrow", "Mañana"),
    ("all day", "todo el día"),
    ("‹ Back", "‹ Atrás"),
    ("All day, {} to {}", "Todo el día, de {} a {}"),
    ("All day, {}", "Todo el día, {}"),
    ("{} to {}", "{} a {}"),
    ("Where: {}", "Dónde: {}"),
    ("charged", "cargada"),
    ("charging", "cargando"),
    ("{}h {}m left", "{} h {} min restantes"),
    ("Battery {} %", "Batería {} %"),
    ("No battery (plugged in)", "Sin batería (conectado a la corriente)"),
    ("Wi-Fi: not connected", "Wi-Fi: sin conexión"),
    ("No Wi-Fi", "Sin Wi-Fi"),
    ("connected", "conectado"),
    ("Bluetooth on", "Bluetooth activado"),
    ("nothing connected", "nada conectado"),
    ("Bluetooth off", "Bluetooth desactivado"),
    ("No Bluetooth", "Sin Bluetooth"),
    ("Browser", "Navegador"),
    ("(no title)", "(sin título)"),
    ("Previous", "Anterior"),
    ("Play", "Reproducir"),
    ("Next", "Siguiente"),
    ("Soft shadow (drawn by Windows)", "Sombra suave (dibujada por Windows)"),
    ("Size", "Tamaño"),
    ("Updates", "Actualizaciones"),
    ("This is version {}.", "Esta es la versión {}."),
    ("Checking…", "Comprobando…"),
    ("Check now", "Comprobar ahora"),
    ("Look for new versions automatically (once a day)", "Buscar nuevas versiones automáticamente (una vez al día)"),
    ("Version {} is available.", "La versión {} está disponible."),
    ("Update now", "Actualizar ahora"),
    ("You have the newest version.", "Tienes la versión más reciente."),
    ("DesktopShepherd {} is available", "DesktopShepherd {} está disponible"),
    ("Later", "Más tarde"),
    ("Update", "Actualizar"),
    ("Use Windows' accent colour for highlights", "Usar el color de énfasis de Windows para los resaltados"),
    ("Darker text on bright wallpapers", "Texto oscuro sobre fondos claros"),
    ("Sync", "Sincronización"),
    ("Keep my boxes and settings in OneDrive (shared between my PCs)", "Guardar mis cajas y la configuración en OneDrive (compartidas entre mis PC)"),
    ("File: {}. Changes from another PC are loaded within half a minute; your layout is backed up first.", "Archivo: {}. Los cambios de otro PC se cargan en medio minuto; antes se guarda una copia de tu diseño."),
    ("OneDrive isn't set up on this PC.", "OneDrive no está configurado en este PC."),
    ("Places", "Lugares"),
    ("Add a place:", "Añadir un lugar:"),
    ("Mostly clear", "Casi despejado"),
    ("Sleet", "Aguanieve"),
    ("Showers", "Chubascos"),
    ("Snow showers", "Chubascos de nieve"),
    ("Feels like {}°", "Sensación {}°"),
    ("Humidity", "Humedad"),
    ("Wind", "Viento"),
    ("Sunrise", "Amanecer"),
    ("Sunset", "Atardecer"),
    ("UV index", "Índice UV"),
    ("Now", "Ahora"),
    ("Compact", "Compacto"),
    ("Standard", "Estándar"),
    ("Detailed", "Detallado"),
    ("Adapts to the box size", "Se adapta al tamaño de la caja"),
    ("Menus and settings:", "Menús y ajustes:"),
    ("Like Windows", "Como Windows"),
    ("Dark", "Oscuro"),
    ("Light", "Claro"),
    ("Raised look", "Aspecto elevado"),
    ("Check once a day for new versions of DesktopShepherd? (Only asks GitHub for the latest version number.)", "¿Buscar una vez al día nuevas versiones de DesktopShepherd? (Solo pregunta a GitHub cuál es el número de la última versión.)"),
    ("Yes", "Sí"),
    ("No", "No"),
    ("Couldn\x27t rename: a file with that name already exists.", "No se pudo cambiar el nombre: ya existe un archivo con ese nombre."),
    ("Couldn\x27t rename: the name has characters Windows doesn\x27t allow.", "No se pudo cambiar el nombre: contiene caracteres que Windows no permite."),
    ("Couldn\x27t rename: Windows refused (the file may be open).", "No se pudo cambiar el nombre: Windows lo rechazó (puede que el archivo esté abierto)."),
    ("The downloaded update could not be verified; opening the release page instead.", "No se pudo verificar la actualización descargada; se abre la página de la versión."),
    ("The calendar is too large; only the first events are shown.", "El calendario es demasiado grande; solo se muestran los primeros eventos."),
    ("Weather: no connection, trying again in a few minutes.", "Tiempo: sin conexión, se reintentará en unos minutos."),
];

pub static CA: &[(&str, &str)] = &[
    ("Automatic (Windows)", "Automàtic (Windows)"),
    ("that link isn't an iCal calendar (.ics)", "aquest enllaç no és un calendari iCal (.ics)"),
    ("that calendar has something DesktopShepherd can't read", "aquest calendari conté alguna cosa que DesktopShepherd no pot llegir"),
    ("the link must start with https://, http:// or webcal://", "l'enllaç ha de començar per https://, http:// o webcal://"),
    ("the link has no server name", "l'enllaç no té nom de servidor"),
    ("Windows' web access could not start", "no s'ha pogut iniciar l'accés web de Windows"),
    ("could not reach {}", "no s'ha pogut connectar amb {}"),
    ("could not prepare the request", "no s'ha pogut preparar la sol·licitud"),
    ("could not reach {} ({})", "no s'ha pogut connectar amb {} ({})"),
    ("no answer from {} ({})", "{} no respon ({})"),
    ("{} refused the link (error {}): is it the private/secret iCal address?", "{} ha rebutjat l'enllaç (error {}): és l'adreça iCal privada/secreta?"),
    ("that is the calendar's public address, which only works for public calendars: use its \"Secret address in iCal format\" instead", "aquesta és l'adreça pública del calendari, que només funciona amb calendaris públics: utilitza la seva \"Adreça secreta en format iCal\""),
    ("{} doesn't know that link (error 404)", "{} no reconeix aquest enllaç (error 404)"),
    ("{} answered with error {}", "{} ha respost amb l'error {}"),
    ("the download was interrupted", "la baixada s'ha interromput"),
    ("that calendar is too big (over 8 MB)", "aquest calendari és massa gran (més de 8 MB)"),
    ("not available on this system", "no disponible en aquest sistema"),
    ("Desktop", "Escriptori"),
    ("New box", "Nova caixa"),
    ("Export layout", "Exporta la disposició"),
    ("Import layout", "Importa la disposició"),
    ("Remove from this box", "Treu d'aquesta caixa"),
    ("Open file location", "Obre la ubicació del fitxer"),
    ("Name", "Nom"),
    ("Unknown widget \"{}\"", "Widget desconegut \"{}\""),
    ("{} items", "{} elements"),
    ("Everything on the desktop is in a box.", "Tot el que hi ha a l'escriptori és en una caixa."),
    ("Drag files here, or right-click an icon › Move to.", "Arrossega fitxers aquí, o fes clic dret en una icona › Mou a."),
    ("Colour", "Color"),
    ("Any colour…", "Qualsevol color…"),
    ("Opacity", "Opacitat"),
    ("Corners", "Cantonades"),
    ("Blur", "Desenfocament"),
    ("Border", "Vora"),
    ("Thickness", "Gruix"),
    ("Border colour…", "Color de la vora…"),
    ("Text", "Text"),
    ("Text colour…", "Color del text…"),
    ("Shadow behind the text", "Ombra darrere del text"),
    ("Drop shadow", "Ombra"),
    ("Title size", "Mida del títol"),
    ("Text size", "Mida del text"),
    ("Font", "Tipus de lletra"),
    ("Icons", "Icones"),
    ("Show names under the icons", "Mostra els noms sota les icones"),
    ("size", "mida"),
    ("auto", "auto"),
    ("Spacing", "Espaiat"),
    ("Title", "Títol"),
    ("Off", "Desactivat"),
    ("Left", "Esquerra"),
    ("Top", "A dalt"),
    ("Bottom", "A baix"),
    ("Tabs", "Pestanyes"),
    ("Right side", "Costat dret"),
    ("Apply this look to all boxes", "Aplica aquest aspecte a totes les caixes"),
    ("Rename box", "Canvia el nom de la caixa"),
    ("Roll up", "Enrotlla"),
    ("Unroll", "Desenrotlla"),
    ("Lock position and size", "Bloqueja la posició i la mida"),
    ("View", "Visualització"),
    ("Sort by", "Ordena per"),
    ("New tab", "Nova pestanya"),
    ("Tab {}", "Pestanya {}"),
    ("Collect other desktop items", "Reuneix la resta d'elements de l'escriptori"),
    ("Shows every desktop item no other box holds, sorted into tabs by kind", "Mostra tots els elements de l'escriptori que no són en una altra caixa, classificats en pestanyes per tipus"),
    ("Personalize…", "Personalitza…"),
    ("Widget settings…", "Configuració del widget…"),
    ("New", "Nou"),
    ("Move to page", "Mou a la pàgina"),
    ("Keyboard shortcut", "Drecera de teclat"),
    ("Search desktop", "Cerca a l'escriptori"),
    ("DesktopShepherd settings", "Configuració de DesktopShepherd"),
    ("Click again to delete (files stay)", "Fes clic de nou per eliminar (els fitxers es conserven)"),
    ("Delete box", "Elimina la caixa"),
    ("Small icons", "Icones petites"),
    ("Medium icons", "Icones mitjanes"),
    ("Large icons", "Icones grans"),
    ("Scroll when icons don't fit", "Desplaça quan les icones no hi caben"),
    ("Pages with dots when icons don't fit", "Pàgines amb punts quan les icones no hi caben"),
    ("As I arrange them", "Tal com els organitzi"),
    ("Item type", "Tipus d'element"),
    ("Date modified", "Data de modificació"),
    ("Times opened", "Cops obert"),
    ("None", "Cap"),
    ("Brings the box into view; pressed again, rolls it up or down.", "Mostra la caixa; si es prem de nou, l'enrotlla o la desenrotlla."),
    ("Page {} (shown now)", "Pàgina {} (visible ara)"),
    ("Page {}", "Pàgina {}"),
    ("New page", "Nova pàgina"),
    ("Switch pages with Ctrl+Alt+PageDown / PageUp or the tray icon.", "Canvia de pàgina amb Ctrl+Alt+PageDown / PageUp o amb la icona de la safata."),
    ("Draw a new box…", "Dibuixa una caixa nova…"),
    ("Drag a rectangle anywhere on the screen (Ctrl+Alt+B)", "Arrossega un rectangle a qualsevol lloc de la pantalla (Ctrl+Alt+B)"),
    ("Box", "Caixa"),
    ("Folder box…", "Caixa de carpeta…"),
    ("A box that shows what is in a folder, live", "Una caixa que mostra el contingut d'una carpeta en temps real"),
    ("{} widget", "Widget {}"),
    ("Rename tab", "Canvia el nom de la pestanya"),
    ("Delete tab", "Elimina la pestanya"),
    ("Its items go back to the desktop", "Els seus elements tornen a l'escriptori"),
    ("Move to", "Mou a"),
    ("Open", "Obre"),
    ("Boxes", "Caixes"),
    ("Layout", "Disposició"),
    ("Settings", "Configuració"),
    ("➕ New box", "➕ Nova caixa"),
    ("➕ New auto-sort box", "➕ Nova caixa de classificació automàtica"),
    ("Collects every desktop item no other box holds, sorted into tabs by kind", "Reuneix tots els elements de l'escriptori que no són en una altra caixa, classificats en pestanyes per tipus"),
    ("➕ New folder box…", "➕ Nova caixa de carpeta…"),
    ("➕ Add widget  {}", "➕ Afegeix widget  {}"),
    ("Recently deleted:", "Eliminades recentment:"),
    ("Bring this box back", "Recupera aquesta caixa"),
    ("{} items on the desktop, {} not in a box yet. Tip: right-click a box for its options; double-click its title to roll it up.", "{} elements a l'escriptori, {} encara sense caixa. Consell: fes clic dret en una caixa per veure'n les opcions; fes doble clic al títol per enrotllar-la."),
    ("Kind", "Tipus"),
    ("Contents", "Contingut"),
    ("State", "Estat"),
    ("Auto-sort box", "Caixa de classificació automàtica"),
    ("{} items in {} tab(s)", "{} elements en {} pestanya/es"),
    ("Rolled up", "Enrotllada"),
    ("Locked", "Bloquejada"),
    ("Confirm delete", "Confirma l'eliminació"),
    ("Delete", "Elimina"),
    ("Main monitor", "Monitor principal"),
    ("Second monitor", "Segon monitor"),
    ("Third monitor", "Tercer monitor"),
    ("Monitor {}", "Monitor {}"),
    ("{} (fixed){}  ·  {}", "{} (fix){}  ·  {}"),
    (", main now", ", principal ara"),
    ("All {} monitors", "Els {} monitors"),
    ("Your desktop, as the boxes sit on it", "El teu escriptori, amb les caixes on són"),
    ("Export layout…", "Exporta la disposició…"),
    ("Save all boxes and settings to a file (it includes your agenda links)", "Desa totes les caixes i la configuració en un fitxer (inclou els teus enllaços d'agenda)"),
    ("Import layout…", "Importa la disposició…"),
    ("Replace the boxes and settings with a saved file (the current ones are backed up first)", "Reemplaça les caixes i la configuració amb un fitxer desat (abans es fa còpia de les actuals)"),
    ("Bring every box back on screen", "Retorna totes les caixes a la pantalla"),
    ("Boxes stay on their display (Settings > Monitors: fixed).", "Les caixes es queden a la seva pantalla (Configuració > Monitors: fix)."),
    ("Boxes follow the monitor's role: main, second… (Settings > Monitors: by role).", "Les caixes segueixen la funció del monitor: principal, segon… (Configuració > Monitors: per funció)."),
    ("No boxes on this monitor yet", "Encara no hi ha caixes en aquest monitor"),
    ("All monitors", "Tots els monitors"),
    ("Language", "Idioma"),
    ("Add \"DesktopShepherd\" to the desktop's right-click menu (new box or widget where you click)", "Afegeix \"DesktopShepherd\" al menú contextual de l'escriptori (caixa o widget nou on facis clic)"),
    ("On Windows 11 it is under \"Show more options\". Or press Ctrl+Alt+B anywhere and drag a rectangle.", "A Windows 11 és a \"Mostra més opcions\". O prem Ctrl+Alt+B a qualsevol lloc i arrossega un rectangle."),
    ("Hide Windows' own desktop icons (the boxes show them)", "Amaga les icones pròpies de l'escriptori de Windows (les caixes les mostren)"),
    ("They always come back when DesktopShepherd closes, even after a crash.", "Sempre tornen en tancar DesktopShepherd, fins i tot després d'una fallada."),
    ("Open rolled-up boxes while the mouse is over them", "Obre les caixes enrotllades mentre el ratolí és a sobre"),
    ("Fade boxes while the mouse is away", "Atenua les caixes mentre el ratolí no és a sobre"),
    ("to", "a"),
    ("after", "després de"),
    ("Snap boxes to each other and to the screen edges", "Ajusta les caixes entre elles i a les vores de la pantalla"),
    ("gap", "separació"),
    ("Also snap to a grid", "Ajusta també a una quadrícula"),
    ("Hold Alt while moving or resizing a box to place it freely.", "Mantén premuda Alt en moure o canviar la mida d'una caixa per col·locar-la lliurement."),
    ("Quick search also finds programs (from the Start menu)", "La cerca ràpida també troba programes (del menú Inici)"),
    ("Quick search shortcut:", "Drecera de teclat de la cerca ràpida:"),
    ("That shortcut is already used by another program: pick another one.", "Aquesta drecera ja la utilitza un altre programa: tria'n una altra."),
    ("Hide / show all boxes:", "Amaga / mostra totes les caixes:"),
    ("Typing on the desktop:", "Escriure a l'escriptori:"),
    ("Does nothing", "No fa res"),
    ("Jumps to the icon that starts with it", "Salta a la icona que comença per això"),
    ("Starts a quick search", "Inicia una cerca ràpida"),
    ("Click an empty part of the desktop, then type: the same letter again goes to the next match, Enter opens it.", "Fes clic en una part buida de l'escriptori i escriu: la mateixa lletra un altre cop va a la següent coincidència; Retorn l'obre."),
    ("Frosted glass: set each box's blur in its menu (Personalize > Blur).", "Vidre esmerilat: ajusta el desenfocament de cada caixa al seu menú (Personalitza > Desenfocament)."),
    ("Monitors", "Monitors"),
    ("Each Windows virtual desktop shows its own page of boxes (desktop 1 = page 1, desktop 2 = page 2…)", "Cada escriptori virtual de Windows mostra la seva pròpia pàgina de caixes (escriptori 1 = pàgina 1, escriptori 2 = pàgina 2…)"),
    ("Remember box positions for each monitor setup (e.g. laptop alone / docked)", "Recorda les posicions de les caixes per a cada configuració de monitors (p. ex., portàtil sol / amb base)"),
    ("Forget", "Oblida"),
    ("Bind each box to:", "Vincula cada caixa a:"),
    ("The monitor's role (main, second…): when the roles change, the boxes move with them", "La funció del monitor (principal, segon…): quan canvien les funcions, les caixes es mouen amb elles"),
    ("The monitor itself: boxes always stay on the monitor where they were created or moved", "El monitor mateix: les caixes sempre es queden al monitor on es van crear o moure"),
    ("Moving a box to another monitor binds it there. Settings > Layout shows each monitor.", "En moure una caixa a un altre monitor, hi queda vinculada. Configuració > Disposició mostra cada monitor."),
    ("{}: {} × {} ({}) — {} box(es)", "{}: {} × {} ({}) — {} caixa/es"),
    ("Auto-sort tabs", "Pestanyes de classificació automàtica"),
    ("Each tab collects items by file type, name pattern (* and ?) and/or how recently they changed; the first matching rule wins. Folders and everything else get their own tabs.", "Cada pestanya reuneix elements per tipus de fitxer, patró de nom (* i ?) i/o per com de recentment han canviat; guanya la primera regla que coincideix. Les carpetes i tota la resta tenen les seves pròpies pestanyes."),
    ("types: zip, rar, 7z", "tipus: zip, rar, 7z"),
    ("name: *invoice*", "nom: *factura*"),
    ("changed in last", "canviat en els últims"),
    (" days", " dies"),
    ("0 = any age", "0 = qualsevol antiguitat"),
    ("Remove", "Treu"),
    ("Add tab rule", "Afegeix una regla de pestanya"),
    ("Reset to defaults", "Restableix els valors predeterminats"),
    ("Computer", "Equip"),
    ("Start DesktopShepherd when Windows starts", "Inicia DesktopShepherd en iniciar Windows"),
    ("Run at low priority (never slows other programs)", "Executa amb prioritat baixa (mai alenteix altres programes)"),
    ("New, renamed and removed desktop files show up as soon as Windows reports them.", "Els fitxers de l'escriptori nous, reanomenats i eliminats apareixen tan bon punt Windows els notifica."),
    ("Saved layouts", "Disposicions desades"),
    ("A copy of your layout is kept each time DesktopShepherd starts (the last 10).", "Es desa una còpia de la teva disposició cada cop que s'inicia DesktopShepherd (les últimes 10)."),
    ("Restore", "Restaura"),
    ("Open data folder", "Obre la carpeta de dades"),
    ("Quit DesktopShepherd", "Surt de DesktopShepherd"),
    ("{} desktop items", "{} elements de l'escriptori"),
    ("Apps", "Aplicacions"),
    ("Clock", "Rellotge"),
    ("Note", "Nota"),
    ("Welcome to DesktopShepherd!\n• Drag a box by its title, resize from its edges.\n• Double-click a title to roll the box up.\n• Right-click a box or an icon for options.", "Us donem la benvinguda a DesktopShepherd!\n• Arrossega una caixa pel seu títol i canvia'n la mida des de les vores.\n• Fes doble clic en un títol per enrotllar la caixa.\n• Fes clic dret en una caixa o una icona per veure'n les opcions."),
    ("Open DesktopShepherd", "Obre DesktopShepherd"),
    ("Hide / show boxes", "Amaga / mostra les caixes"),
    ("Next page of boxes", "Pàgina següent de caixes"),
    ("Draw a new box…  (Ctrl+Alt+B)", "Dibuixa una caixa nova…  (Ctrl+Alt+B)"),
    ("Bring back the last deleted box", "Recupera l'última caixa eliminada"),
    ("Quit", "Surt"),
    ("Search the desktop…", "Cerca a l'escriptori…"),
    ("Type part of a name. Enter opens, Ctrl+Enter opens its folder, Esc closes.", "Escriu part d'un nom. Retorn obre, Ctrl+Retorn obre la seva carpeta, Esc tanca."),
    ("Nothing on the desktop matches.", "Res a l'escriptori coincideix."),
    ("Program", "Programa"),
    ("Rename", "Canvia el nom"),
    ("Enter renames, Esc cancels.", "Retorn canvia el nom, Esc cancel·la."),
    ("{}  Search the desktop", "{}  Cerca a l'escriptori"),
    ("Nothing found", "No s'ha trobat res"),
    ("Personalize", "Personalitza"),
    ("Look", "Aspecte"),
    ("Widget settings", "Configuració del widget"),
    ("Done", "Fet"),
    ("“{}” deleted", "“{}” eliminada"),
    ("Undo", "Desfés"),
    ("Choose a folder for the box", "Tria una carpeta per a la caixa"),
    ("Choose a picture", "Tria una imatge"),
    ("Pictures (PNG, JPEG)", "Imatges (PNG, JPEG)"),
    ("New box here", "Nova caixa aquí"),
    ("New folder box here…", "Nova caixa de carpeta aquí…"),
    ("New widget here", "Nou widget aquí"),
    ("DesktopShepherd layout (*.json)", "Disposició de DesktopShepherd (*.json)"),
    ("Drag to draw the new box  ·  Esc to cancel", "Arrossega per dibuixar la caixa nova  ·  Esc per cancel·lar"),
    ("Widget", "Widget"),
    ("Update {}?\n\nThe installed copy in\n{}\nwill be replaced. Your boxes stay as they are.", "Voleu actualitzar {}?\n\nLa còpia instal·lada a\n{}\ns'ha de reemplaçar. Les teves caixes es queden tal com són."),
    ("Install {}?\n\nIt goes to\n{}\n(just for you, no admin rights needed), gets a Start menu entry and starts with Windows.\n\nYou can remove it later from Settings > Apps > Installed apps.", "Voleu instal·lar {}?\n\nS'instal·larà a\n{}\n(només per a tu, sense permisos d'administrador), tindrà una entrada al menú Inici i s'iniciarà amb Windows.\n\nEl pots treure més tard des de Configuració > Aplicacions > Aplicacions instal·lades."),
    ("{} is still running and didn't close. Quit it from its tray icon, then run the setup again.", "{} encara s'està executant i no s'ha tancat. Surt des de la seva icona de la safata i torna a executar la instal·lació."),
    ("Could not copy the program to\n{}\n\n{}", "No s'ha pogut copiar el programa a\n{}\n\n{}"),
    ("{} is updated. It starts when you press OK; its icon is in the tray (next to the clock).", "{} s'ha actualitzat. S'inicia en prémer D'acord; la seva icona és a la safata (al costat del rellotge)."),
    ("{} is installed. It starts when you press OK; its icon is in the tray (next to the clock).", "{} s'ha instal·lat. S'inicia en prémer D'acord; la seva icona és a la safata (al costat del rellotge)."),
    ("{} is installed but did not start: {}", "{} s'ha instal·lat però no s'ha iniciat: {}"),
    ("Remove {} from this computer?", "Voleu treure {} d'aquest equip?"),
    ("{} is still running and didn't close. Quit it from its tray icon, then try again.", "{} encara s'està executant i no s'ha tancat. Surt des de la seva icona de la safata i torna-ho a provar."),
    ("Also delete your saved boxes and settings?\n\n{}\n\nChoose No to keep them in case you install {} again.", "Voleu eliminar també les teves caixes i configuració desades?\n\n{}\n\nTria No per conservar-les per si tornes a instal·lar {}."),
    ("{} has been removed.", "{} s'ha tret."),
    ("Work", "Feina"),
    ("Break", "Descans"),
    ("Time's up!", "S'ha acabat el temps!"),
    ("Timer", "Temporitzador"),
    ("Pause", "Pausa"),
    ("Resume", "Represa"),
    ("Start", "Inicia"),
    ("Reset", "Restableix"),
    ("{} min", "{} min"),
    ("Pomodoro: 25 minutes of work, 5 of rest, a longer rest every fourth time", "Pomodoro: 25 minuts de feina, 5 de descans, i un descans més llarg cada quart cop"),
    ("Countdown length", "Durada del compte enrere"),
    ("Downloading the update…", "Baixant l'actualització…"),
    ("Could not save the update", "No s'ha pogut desar l'actualització"),
    ("Could not start the update", "No s'ha pogut iniciar l'actualització"),
    ("The downloaded file is not a program; opening the release page instead.", "El fitxer baixat no és un programa; s'obre la pàgina de la versió en el seu lloc."),
    ("Could not download the update", "No s'ha pogut baixar l'actualització"),
    ("Clear", "Serè"),
    ("Partly cloudy", "Parcialment ennuvolat"),
    ("Cloudy", "Ennuvolat"),
    ("Fog", "Boira"),
    ("Drizzle", "Plugim"),
    ("Rain", "Pluja"),
    ("Snow", "Neu"),
    ("Thunderstorm", "Tempesta"),
    ("Right-click here and choose \"Widget settings…\" to pick a place.", "Fes clic dret aquí i tria \"Configuració del widget…\" per triar un lloc."),
    ("Loading…", "Carregant…"),
    ("Today", "Avui"),
    ("Place", "Lloc"),
    ("Type a town or city", "Escriu una localitat o ciutat"),
    ("Search", "Cerca"),
    ("Nothing found for \"{}\".", "No s'ha trobat res per a \"{}\"."),
    ("Weather data by Open-Meteo.com, updated every 30 minutes while the widget is shown.", "Dades meteorològiques d'Open-Meteo.com, actualitzades cada 30 minuts mentre es mostra el widget."),
    ("24-hour", "24 hores"),
    ("24-hour clock", "Rellotge de 24 hores"),
    ("Type a note…", "Escriu una nota…"),
    ("{} GB free", "{} GB lliures"),
    ("Back to today", "Torna a avui"),
    ("Week starts on Sunday", "La setmana comença en diumenge"),
    ("Add a task, then Enter", "Afegeix una tasca i prem Retorn"),
    ("Remove finished tasks", "Treu les tasques acabades"),
    ("{} left", "{} pendents"),
    ("Right-click here and choose \"Widget settings…\" to add a calendar link (iCal / .ics).", "Fes clic dret aquí i tria \"Configuració del widget…\" per afegir un enllaç de calendari (iCal / .ics)."),
    ("Google Calendar: Settings > your calendar > \"Secret address in iCal format\". Outlook: Settings > Calendar > Shared calendars > Publish.", "Google Calendar: Configuració > el teu calendari > \"Adreça secreta en format iCal\". Outlook: Configuració > Calendari > Calendaris compartits > Publica."),
    ("⚠ {} calendar(s) not updated: {}", "⚠ {} calendari(s) sense actualitzar: {}"),
    ("Month", "Mes"),
    ("Week", "Setmana"),
    ("Agenda", "Agenda"),
    ("Calendar links (iCal / .ics)", "Enllaços de calendari (iCal / .ics)"),
    ("The full link stays on this PC only: in your layout file and its backups.", "L'enllaç complet només es queda en aquest PC: al teu fitxer de disposició i les seves còpies de seguretat."),
    ("Paste a link: https://… or webcal://…", "Enganxa un enllaç: https://… o webcal://…"),
    ("Add", "Afegeix"),
    ("Show the next", "Mostra els propers"),
    ("{} days", "{} dies"),
    ("Refresh now", "Actualitza ara"),
    ("Calendars are downloaded every 30 minutes while this widget is shown.", "Els calendaris es baixen cada 30 minuts mentre es mostra aquest widget."),
    ("can't open the picture ({})", "no es pot obrir la imatge ({})"),
    ("can't read that picture", "no es pot llegir aquesta imatge"),
    ("Right-click here and choose \"Widget settings…\" to pick a picture.", "Fes clic dret aquí i tria \"Configuració del widget…\" per triar una imatge."),
    ("Picture (PNG or JPEG)", "Imatge (PNG o JPEG)"),
    ("Choose…", "Tria…"),
    ("Show the whole picture", "Mostra la imatge completa"),
    ("Fill the box (edges may be cut)", "Omple la caixa (les vores es poden retallar)"),
    ("Today, {}", "Avui, {}"),
    ("Tomorrow, {}", "Demà, {}"),
    ("Nothing in the next {} days.", "Res en els propers {} dies."),
    ("Tomorrow", "Demà"),
    ("all day", "tot el dia"),
    ("‹ Back", "‹ Enrere"),
    ("All day, {} to {}", "Tot el dia, de {} a {}"),
    ("All day, {}", "Tot el dia, {}"),
    ("{} to {}", "{} a {}"),
    ("Where: {}", "On: {}"),
    ("charged", "carregada"),
    ("charging", "carregant"),
    ("{}h {}m left", "{} h {} min restants"),
    ("Battery {} %", "Bateria {} %"),
    ("No battery (plugged in)", "Sense bateria (connectat al corrent)"),
    ("Wi-Fi: not connected", "Wi-Fi: sense connexió"),
    ("No Wi-Fi", "Sense Wi-Fi"),
    ("connected", "connectat"),
    ("Bluetooth on", "Bluetooth activat"),
    ("nothing connected", "res connectat"),
    ("Bluetooth off", "Bluetooth desactivat"),
    ("No Bluetooth", "Sense Bluetooth"),
    ("Browser", "Navegador"),
    ("(no title)", "(sense títol)"),
    ("Previous", "Anterior"),
    ("Play", "Reprodueix"),
    ("Next", "Següent"),
    ("Soft shadow (drawn by Windows)", "Ombra suau (dibuixada per Windows)"),
    ("Size", "Mida"),
    ("Updates", "Actualitzacions"),
    ("This is version {}.", "Aquesta és la versió {}."),
    ("Checking…", "S'està comprovant…"),
    ("Check now", "Comprova-ho ara"),
    ("Look for new versions automatically (once a day)", "Cerca versions noves automàticament (un cop al dia)"),
    ("Version {} is available.", "La versió {} està disponible."),
    ("Update now", "Actualitza ara"),
    ("You have the newest version.", "Tens la versió més recent."),
    ("DesktopShepherd {} is available", "DesktopShepherd {} està disponible"),
    ("Later", "Més tard"),
    ("Update", "Actualitza"),
    ("Use Windows' accent colour for highlights", "Utilitza el color d'èmfasi de Windows per als ressaltats"),
    ("Darker text on bright wallpapers", "Text fosc sobre fons clars"),
    ("Sync", "Sincronització"),
    ("Keep my boxes and settings in OneDrive (shared between my PCs)", "Desa les meves caixes i la configuració a OneDrive (compartides entre els meus PC)"),
    ("File: {}. Changes from another PC are loaded within half a minute; your layout is backed up first.", "Fitxer: {}. Els canvis d'un altre PC es carreguen en mig minut; abans es desa una còpia del teu disseny."),
    ("OneDrive isn't set up on this PC.", "OneDrive no està configurat en aquest PC."),
    ("Places", "Llocs"),
    ("Add a place:", "Afegeix un lloc:"),
    ("Mostly clear", "Gairebé serè"),
    ("Sleet", "Aiguaneu"),
    ("Showers", "Ruixats"),
    ("Snow showers", "Ruixats de neu"),
    ("Feels like {}°", "Sensació {}°"),
    ("Humidity", "Humitat"),
    ("Wind", "Vent"),
    ("Sunrise", "Sortida del sol"),
    ("Sunset", "Posta de sol"),
    ("UV index", "Índex UV"),
    ("Now", "Ara"),
    ("Compact", "Compacte"),
    ("Standard", "Estàndard"),
    ("Detailed", "Detallat"),
    ("Adapts to the box size", "S'adapta a la mida de la caixa"),
    ("Menus and settings:", "Menús i configuració:"),
    ("Like Windows", "Com Windows"),
    ("Dark", "Fosc"),
    ("Light", "Clar"),
    ("Raised look", "Aspecte elevat"),
    ("Check once a day for new versions of DesktopShepherd? (Only asks GitHub for the latest version number.)", "Cercar una vegada al dia noves versions de DesktopShepherd? (Només pregunta a GitHub quin és el número de l\x27última versió.)"),
    ("Yes", "Sí"),
    ("No", "No"),
    ("Couldn\x27t rename: a file with that name already exists.", "No s\x27ha pogut canviar el nom: ja existeix un fitxer amb aquest nom."),
    ("Couldn\x27t rename: the name has characters Windows doesn\x27t allow.", "No s\x27ha pogut canviar el nom: conté caràcters que Windows no permet."),
    ("Couldn\x27t rename: Windows refused (the file may be open).", "No s\x27ha pogut canviar el nom: Windows l\x27ha rebutjat (potser el fitxer és obert)."),
    ("The downloaded update could not be verified; opening the release page instead.", "No s\x27ha pogut verificar l\x27actualització baixada; s\x27obre la pàgina de la versió."),
    ("The calendar is too large; only the first events are shown.", "El calendari és massa gran; només es mostren els primers esdeveniments."),
    ("Weather: no connection, trying again in a few minutes.", "Temps: sense connexió, es tornarà a provar d\x27aquí a uns minuts."),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> usize {
        s.matches("{}").count()
    }

    #[test]
    fn tables_are_consistent() {
        use std::collections::HashSet;
        for (name, table) in [("ES", ES), ("CA", CA)] {
            let mut seen = HashSet::new();
            for (k, v) in table.iter() {
                assert!(seen.insert(*k), "{name}: duplicate key {k:?}");
                assert_eq!(placeholders(k), placeholders(v), "{name}: {{}} count differs for {k:?}");
            }
        }
        let es: HashSet<&str> = ES.iter().map(|(k, _)| *k).collect();
        let ca: HashSet<&str> = CA.iter().map(|(k, _)| *k).collect();
        for k in &es {
            assert!(ca.contains(k), "missing in CA: {k:?}");
        }
        for k in &ca {
            assert!(es.contains(k), "missing in ES: {k:?}");
        }
    }
    #[test]
    fn placeholders_and_dates() {
        LANG.store(1, Ordering::Relaxed);
        assert_eq!(t("Language"), "Idioma");
        assert_eq!(t("not translated"), "not translated");
        let d = chrono::NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        assert_eq!(long_date(d), "domingo, 22 de noviembre");
        LANG.store(2, Ordering::Relaxed);
        assert_eq!(month_year(d), "novembre de 2026");
        let d = chrono::NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        assert_eq!(long_date(d), "dilluns, 3 d'agost");
        LANG.store(0, Ordering::Relaxed);
        assert_eq!(tf("{} of {}", &[&1, &2]), "1 of 2");
    }
}
