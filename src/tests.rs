
#[test]
fn it_works() {
    assert_eq!(2 + 2, 4);
}

#[test]
fn task() {

    use crate::*;

    spawn_task(async || {
        let mut num = 0i32;
        let task = spawn_task(async || {
            if num == 0 {
                num = 1;
                task::yield_now().await;
                num = 0;
                true
            } else {
                false
            }
        });

        task::yield_now().await;

        task.await;

        //let t = task.await;
        //assert_eq!(t, true);



        assert_eq!(num, 0);



        post_quit();
    }).detach();

    run();


}