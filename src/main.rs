use futures::StreamExt;
use std::env;
use telegram_bot::*;
use yeelight::{
    AdjustAction, Bulb, CfAction, Effect, FlowExpresion, FlowTuple, Mode, Power, Properties,
    Property,
};

#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv()?;
    let my_bulb_ip = "192.168.88.96";
    let mut bulb = Bulb::connect(my_bulb_ip, 55443).await?;

    let token = env::var("TELEGRAM_TOKEN").expect("Token not present");
    let api = Api::new(token);

    // Fetch new updates via long poll method
    let mut stream = api.stream();
    while let Some(update) = stream.next().await {
        // If the received update contains a new message...
        let update = update?;
        if let UpdateKind::Message(message) = update.kind {
            if let MessageKind::Text { ref data, .. } = message.kind {
                // Print received text message to stdout.
                println!("<{}>: {}", &message.from.id, data);

                let user_id: i64 = message.from.id.into();
                if
                /*user_id == 801021640 || */
                user_id == 486433660 {
                    let props = Properties(vec![Property::Power]);

                    let state = bulb.get_prop(&props).await?;

                    println!("state is {:?}", state);
                    if let Some(props) = state {
                        if props.contains(&"on".to_string()) {
                            api.send(message.chat.text(format!(
                                "Greetings, {}. Turning light off",
                                &message.from.first_name
                            )))
                            .await?;

                            let response = bulb
                                .set_power(Power::Off, Effect::Smooth, 1, Mode::Normal)
                                .await?;
                            println!("response: {:?}", response);
                        } else {
                            api.send(message.chat.text(format!(
                                "Greetings, {}. Turning light on",
                                &message.from.first_name
                            )))
                            .await?;

                            bulb.set_power(Power::On, Effect::Smooth, 3, Mode::CT)
                                .await?;

                            bulb.set_bright(50, Effect::Smooth, 3).await?;
                            bulb.set_ct_abx(3500, Effect::Smooth, 3).await?;
                        }
                    }
                } else {
                    api.send(message.text_reply(format!(
                        "Greetings, {}. I am a private bot and you are not authorized to interact with me",
                        &message.from.first_name
                    )))
                    .await?;
                }
            }
        }
    }

    Ok(())
}
