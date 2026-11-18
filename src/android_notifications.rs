//! Android OS alerts run independently of UI redraws, on a foreground service thread.
use jni::{
    JValue, jni_sig, jni_str,
    objects::{JObject, JString},
    refs::Global,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use tungstenite::Message;

pub fn alert(title: &str, body: &str, sound: bool) -> Result<(), String> {
    let app = crate::platform::android_app().ok_or("Android activity unavailable")?;
    let activity_app = app.clone();
    let title = title.to_owned();
    let body = body.to_owned();
    app.run_on_java_main_thread(Box::new(move || {
        let vm = unsafe { jni::JavaVM::from_raw(activity_app.vm_as_ptr().cast()) };
        let _ = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let raw = activity_app.activity_as_ptr() as jni::sys::jobject;
            let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw)? };
            let title = env.new_string(title)?;
            let body = env.new_string(body)?;
            env.call_method(
                activity.as_ref(),
                jni_str!("notifyLocal"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;Z)V"),
                &[
                    JValue::Object(&title),
                    JValue::Object(&body),
                    JValue::Bool(sound),
                ],
            )?;
            Ok(())
        });
    }));
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_site_blig_d4c_D4cNotificationService_watchNotifications<'a>(
    mut env: jni::EnvUnowned<'a>,
    service: JObject<'a>,
    storage: JString<'a>,
) {
    env.with_env(|env|->jni::errors::Result<()>{
        crate::platform::set_android_storage(std::path::PathBuf::from(storage.try_to_string(env)?));
        let _=rustls::crypto::ring::default_provider().install_default();
        while env.call_method(&service,jni_str!("isRunning"),jni_sig!("()Z"),&[])?.z()? {
            let settings=crate::preferences::Settings::load();let gateway=settings.current().url.clone();
            let filename=format!("push-{:x}.json",Sha256::digest(gateway.as_bytes()));
            let saved=crate::preferences::read(&filename);
            let mut seen:HashSet<String>=saved.as_array().into_iter().flatten().filter_map(|id|id.as_str().map(str::to_owned)).take(1000).collect();
            let mut initial=saved.is_null();
            if let Some(base)=crate::workspace::base(&gateway){
                if let Ok(mut socket)=crate::workspace::open(&format!("{base}/control")){
                    if let Some(tcp)=crate::workspace::tcp(&mut socket){let _=tcp.set_read_timeout(Some(Duration::from_secs(2)));}
                    let mut checked=Instant::now();
                    loop{
                        if !env.call_method(&service,jni_str!("isRunning"),jni_sig!("()Z"),&[])?.z()?{return Ok(());}
                        if checked.elapsed()>Duration::from_secs(5){checked=Instant::now();if crate::preferences::Settings::load().current().url!=gateway{break;}}
                        match socket.read(){
                            Ok(Message::Text(text))=>{
                                let Ok(value)=serde_json::from_str::<Value>(&text)else{continue;};
                                let p=&value["payload"];
                                let snapshot=if value["type"]=="snapshot"{Some(p)}else if p["resync"]==true{Some(&p["snapshot"])}else{None};
                                let rows:Vec<Value>=if let Some(snapshot)=snapshot{snapshot["notification"].as_array().cloned().unwrap_or_default()}else{p["events"].as_array().into_iter().flatten().filter(|e|e["kind"]=="notification").map(|e|{let mut n=e["value"].clone();n["id"]=e["id"].clone();n}).collect()};
                                let mut changed=false;
                                for row in rows{
                                    let Some(id)=row["id"].as_str()else{continue;};
                                    if !seen.insert(id.into()){continue;}changed=true;
                                    let settings=crate::preferences::Settings::load();
                                    if !initial&&row["unread"]==true&&row["deleted"]!=true&&settings.desktop {
                                        env.with_local_frame(8,|env|->jni::errors::Result<()>{
                                        let title=env.new_string(row["title"].as_str().unwrap_or("D4C"))?;
                                        let body=env.new_string(row["body"].as_str().unwrap_or("").chars().take(500).collect::<String>())?;
                                        let session=env.new_string(row["session"].as_str().unwrap_or(""))?;let id=env.new_string(id)?;
                                        env.call_method(&service,jni_str!("postAlert"),jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Z)V"),&[JValue::Object(&id),JValue::Object(&title),JValue::Object(&body),JValue::Object(&session),JValue::Bool(settings.sound)])?;
                                            Ok(())
                                        })?;
                                    }
                                }
                                if snapshot.is_some(){initial=false;}
                                if changed||snapshot.is_some(){if seen.len()>1000{seen=seen.into_iter().take(800).collect();}let _=crate::preferences::write(&filename,&serde_json::json!(seen));}
                            },
                            Ok(Message::Close(_))=>break,Ok(_)=>{},
                            Err(tungstenite::Error::Io(e))if matches!(e.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::TimedOut)=>{},Err(_)=>break,
                        }
                    }
                }
            }
            for _ in 0..15{if !env.call_method(&service,jni_str!("isRunning"),jni_sig!("()Z"),&[])?.z()?{return Ok(());}std::thread::sleep(Duration::from_millis(500));}
        }Ok(())
    }).resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

static SESSION: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
pub fn take_session() -> Option<String> {
    SESSION.lock().ok().and_then(|mut s| s.take())
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_site_blig_d4c_D4cActivity_openSession<'a>(
    mut env: jni::EnvUnowned<'a>,
    _activity: JObject<'a>,
    session: JString<'a>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let value = session.try_to_string(env)?;
        if !value.is_empty() && value.len() <= 512 {
            if let Ok(mut pending) = SESSION.lock() {
                *pending = Some(value);
            }
        }
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
